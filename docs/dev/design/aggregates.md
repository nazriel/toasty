# Aggregates and Grouping

## Summary

Queries gain aggregate expressions (`count_all()`, `count(f)`, `sum(f)`,
`avg(f)`, `min(f)`, `max(f)`), `.group_by(...)`, and `.having(...)`. An
aggregate is a typed `Expr<T>` that `.select(...)` projects like a field
path, so a report is one list query that returns one row per group,
computed in the database. A field path through `belongs_to` relations
(`Ticket::fields().assignee().name()`) is read with a join in the same
statement, so a query can also group by a related record's field.

## Motivation

[#421] lists the use cases: dashboard counts, revenue per status, the
customers above a threshold (`HAVING`), the latest login per user, a price
range per category. Today `.count()` is the only aggregate, and an
application that needs any of the others either loads every record and
aggregates in Rust or writes raw SQL and decodes rows by hand. Both lose
the typed query builder for the queries that summarize the most data.

Grouping usually happens on a related record's field ("tickets per
assignee's name"). Projecting such a field today issues a second query per
relation and merges the rows in memory, which grouping cannot use.

## User-facing API

### Aggregate functions

The functions live in `toasty::stmt`. Each takes a field path and returns a
typed expression:

| Function | SQL | Type | Over no values |
|---|---|---|---|
| `count_all()` | `COUNT(*)` | `u64` | `0` |
| `count(field)` | `COUNT(field)` | `u64` | `0` |
| `sum(field)` | `SUM(field)` | `Option<T>` | `None` |
| `avg(field)` | `AVG(field)` | `Option<f64>` | `None` |
| `min(field)` | `MIN(field)` | `Option<T>` | `None` |
| `max(field)` | `MAX(field)` | `Option<T>` | `None` |

`T` is the field's type without its `Option` (`Field::Inner`). `sum` and
`avg` require `T: Numeric`. `count` is a `u64`, the type `.count()` already
returns. `avg` of an integer field is an `f64`, as in SQL.

Without `.group_by(...)`, aggregates summarize every matching record in
exactly one row:

```rust
use toasty::stmt::{avg, count_all, sum};

let summary: Vec<(u64, Option<i64>, Option<f64>)> = Order::filter(Order::fields().status().eq("paid"))
    .select((count_all(), sum(Order::fields().total()), avg(Order::fields().total())))
    .exec(&mut db)
    .await?;
```

### Grouping

`.group_by(keys)` takes a field path or a tuple of paths. Each group yields
one row; the projection reads grouping keys and aggregates:

```rust
let per_status: Vec<(String, u64, Option<i64>)> = Order::all()
    .group_by(Order::fields().status())
    .select((Order::fields().status(), count_all(), sum(Order::fields().total())))
    .exec(&mut db)
    .await?;
```

`.having(predicate)` filters groups and may compare aggregates; repeated
calls combine with AND. Aggregates order and limit like any expression:

```rust
let top: Vec<(String, Option<i64>)> = Order::all()
    .group_by(Order::fields().customer())
    .having(sum(Order::fields().total()).gt(1000))
    .select((Order::fields().customer(), sum(Order::fields().total())))
    .order_by(sum(Order::fields().total()).desc().nulls_last())
    .limit(10)
    .exec(&mut db)
    .await?;
```

### A related record's field

A path through `belongs_to` relations can be projected, grouped by,
aggregated, and ordered by. A projected embedded field of the related
record (an enum, a struct, or a field inside a struct) is read by the same
join; one that holds deferred fields or relations is read by the related
record's own statement. For an optional relation the path keeps the
field's type, so `.nullable()` reads it as an `Option`:

```rust
let per_assignee: Vec<(Option<String>, u64)> = Ticket::all()
    .group_by(Ticket::fields().assignee().name().nullable())
    .select((Ticket::fields().assignee().name().nullable(), count_all()))
    .exec(&mut db)
    .await?;
```

## Behavior

**Happy path.** The query is one SQL statement:
`SELECT keys, aggregates FROM t [JOIN ...] WHERE filter GROUP BY keys HAVING
predicate ORDER BY ... LIMIT ...`. A `belongs_to` path adds an `INNER JOIN`
when every relation on it is required and a `LEFT JOIN` once one is
optional; a path used twice joins once.

**Validation, before the query runs** (all `invalid_statement`):

- A grouped or aggregating query reads a field outside an aggregate that is
  not a grouping key, in `.select(...)`, `.having(...)`, or `.order_by(...)`.
  Without `.group_by(...)` the whole query is one group, so `select((name,
  count_all()))` is rejected. This is checked at verification, not at
  compile time: the typed builder does not track which fields are grouped.
- An aggregate inside `.filter(...)` (use `.having(...)`).
- A grouped query without `.select(...)`; it cannot return model records.
- A grouping key that is not a primitive field or a unit-only enum, of the
  model or of a record reached through `belongs_to` relations, directly or
  inside embedded structs (`Ticket::fields().site().city()`). A unit-only
  enum groups by its discriminant column. An enum with data variants or a
  whole embedded struct is not a grouping key: its value spans several
  columns.
- An aggregate argument that is not a primitive field of the model or of a
  record reached through `belongs_to` relations.

**Errors from the backend.** `unsupported_feature` on DynamoDB (see below)
and for cursor pagination (`.paginate(...)`) of a grouped or aggregating
query; `.limit(...)`/`.offset(...)` work. A sum that overflows the field's
type is the database's error.

**Defaults.** Aggregates skip `NULL` values. An aggregate without grouping
returns its row even when the filter is known to match nothing (`IN ()`).

## Edge cases

- `min`/`max` of a field stored in another type than its Rust type (a date
  stored as text on SQLite) compare the stored values and decode the
  result; text dates sort the same as dates.
- A grouping key with `NULL` values forms one `NULL` group, ordered with
  `.nulls_first()`/`.nulls_last()` as usual.
- `.count()` remains the shortcut for `select(count_all())` on an ungrouped
  query.
- A field read through an optional relation without `.nullable()` fails to
  decode on the rows without a related record; this is the existing
  behavior of projecting a nullable association (see
  [column projection](./column-projection.md)).

## Driver integration

- **SQL drivers.** No new `Operation`. `stmt::Select` gains `group_by:
  Vec<Expr>` and `having: Option<Expr>`, and `ExprFunc` gains
  `Aggregate(FuncAggregate { func: Sum | Avg | Min | Max, arg })`;
  `FuncCount { arg: Some(..) }` is serialized as `COUNT(arg)`. The engine
  sends the aggregates in the statement's projection and expects them back
  in the type it decodes, so the serializer casts where a database widens:
  - SQLite: no cast.
  - PostgreSQL: `CAST(SUM(x) AS <column type>)` for integer columns
    (PostgreSQL sums `BIGINT` as `NUMERIC`), `CAST(AVG(x) AS DOUBLE
    PRECISION)`.
  - MySQL/MariaDB: `CAST(SUM(x) AS SIGNED|UNSIGNED)` for integer columns
    (summed as `DECIMAL`), `CAST(AVG(x) AS DOUBLE)`.
- **DynamoDB.** Verification rejects aggregates, `group_by`, and `having`
  with `unsupported_feature`, gated on `Capability::sql`. No new capability
  flag: DynamoDB's `Select: COUNT` covers only counting and would need its
  own design.
- **Out-of-tree drivers.** A SQL driver built on `toasty-sql` gets the new
  clauses from the serializer. A driver that matches `ExprFunc` exhaustively
  must handle the new variant.

## Alternatives considered

**Aggregate methods on the query (`.sum(f)`, `.avg(f)`) returning a
scalar.** Covers the ungrouped case only; grouping would then need a second
surface. Expressions in `.select(...)` cover both, compose in tuples, and
reuse `having`/`order_by`.

**Aggregate methods on paths (`Order::fields().total().sum()`).** Reads
well, but `count_all()` has no path, and free functions match how the
upstream projection design spells `count(F)`.

**Compile-time grouping validation.** Tracking grouped fields in the query
type would make every grouped query's type spell its keys. Verification
reports the same mistake before any SQL runs.

**`LEFT JOIN` for every `belongs_to` path.** Keeps the row count when a
required relation's record is missing, but then a required path's value
could be `NULL` with a non-`Option` type. `INNER JOIN` matches the declared
cardinality.

## Open questions

- **`.nullable()` vs typed nullable paths.** The column-projection design
  plans for a path through a nullable association to be typed `Option<T>`
  by the generated accessors. `.nullable()` is the explicit spelling until
  then. *Deferrable.*
- **`count_distinct(field)`, `string_agg`/`group_concat`, statistical
  aggregates.** Same shape, more functions. *Deferrable.*
- **Aggregates over expressions (`sum(price * quantity)`).** Needs
  arithmetic expressions. *Deferrable.*
- **DynamoDB counts.** `Select: COUNT` on a query or scan could serve
  `count_all()` without grouping. *Deferrable.*

## Out of scope

- Window functions (`OVER (...)`); a separate design.
- Relation aggregates attached to each parent (a child count per post);
  covered by the planned relation-aggregate surface.
- Paths through `has_many`/`has_one` in projections; the column-projection
  design covers them.
- Grouping by an enum with data variants or by a whole embedded struct.

[#421]: https://github.com/tokio-rs/toasty/issues/421
