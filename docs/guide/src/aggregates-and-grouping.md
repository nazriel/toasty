# Aggregates and Grouping

An aggregate summarizes many records in one value: how many orders there
are, the revenue of each customer, the latest login of each user. Toasty
computes aggregates in the database, so a report reads a few summary rows
instead of every record.

Aggregates, `group_by`, and `having` are supported on SQL databases
(SQLite, PostgreSQL, MySQL). DynamoDB rejects them with an
`unsupported_feature` error.

## Aggregate functions

The functions live in `toasty::stmt`. Each takes a field path and returns
a typed expression that you pass to `.select()`:

| Function | SQL | Rust type | Over no values |
|---|---|---|---|
| `count_all()` | `COUNT(*)` | `u64` | `0` |
| `count(field)` | `COUNT(field)` | `u64` | `0` |
| `sum(field)` | `SUM(field)` | `Option<T>` | `None` |
| `avg(field)` | `AVG(field)` | `Option<f64>` | `None` |
| `min(field)` | `MIN(field)` | `Option<T>` | `None` |
| `max(field)` | `MAX(field)` | `Option<T>` | `None` |

`T` is the field's type without its `Option`. `sum` and `avg` take numeric
fields; the sum keeps the field's type, and the mean is an `f64` even for an
integer field. `min` and `max` work on any field the database can compare,
such as strings and dates. Aggregates skip `NULL` values: `count(field)`
counts the records where the field has a value, and a `sum` over records
that all lack a value is `None`.

Without `group_by`, the aggregates summarize every record the query
matches and the result has exactly one row, even when no record matches:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Order {
#     #[key]
#     #[auto]
#     id: u64,
#     status: String,
#     total: i64,
#     discount: Option<i64>,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::{avg, count, count_all, max, sum};

let summary: Vec<(u64, u64, Option<i64>, Option<f64>, Option<i64>)> =
    Order::filter(Order::fields().status().eq("paid"))
        .select((
            count_all(),
            count(Order::fields().discount()),
            sum(Order::fields().total()),
            avg(Order::fields().total()),
            max(Order::fields().total()),
        ))
        .exec(&mut db)
        .await?;

let (orders, discounted, revenue, mean, largest) = summary[0];
# let _ = (orders, discounted, revenue, mean, largest);
# Ok(())
# }
```

To count the records a query matches, `.count()` is the shorter spelling of
`.select(count_all())` and returns the `u64` directly.

## Grouping with `group_by`

`.group_by()` splits the matching records into groups that share the same
value of one or more fields, and the query returns one row per group. Pass
a field path, or a tuple of paths to group by several fields. The
projection reads the grouping fields and aggregates over each group:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Order {
#     #[key]
#     #[auto]
#     id: u64,
#     status: String,
#     region: String,
#     total: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::{count_all, sum};

// Orders and revenue per status.
let per_status: Vec<(String, u64, Option<i64>)> = Order::all()
    .group_by(Order::fields().status())
    .select((
        Order::fields().status(),
        count_all(),
        sum(Order::fields().total()),
    ))
    .exec(&mut db)
    .await?;

// Per status and region.
let per_status_and_region: Vec<(String, String, u64)> = Order::all()
    .group_by((Order::fields().status(), Order::fields().region()))
    .select((
        Order::fields().status(),
        Order::fields().region(),
        count_all(),
    ))
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

Everything a grouped query reads outside an aggregate must be one of its
grouping fields, because a group has one value only for those. Toasty checks
this before it sends the query: selecting, filtering groups, or sorting by
another field returns an `invalid_statement` error. The same applies to a
query that aggregates without `group_by`, which forms one group of every
record. A grouped query must call `.select()`; it cannot return model
records.

## Filtering groups with `having`

`.filter()` keeps or drops individual records before they are grouped.
`.having()` keeps or drops whole groups after grouping, so its condition can
compare aggregates:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Order {
#     #[key]
#     #[auto]
#     id: u64,
#     customer: String,
#     status: String,
#     total: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::sum;

// Customers whose paid orders add up to more than 1000.
let big_spenders: Vec<(String, Option<i64>)> = Order::filter(Order::fields().status().eq("paid"))
    .group_by(Order::fields().customer())
    .having(sum(Order::fields().total()).gt(1000))
    .select((Order::fields().customer(), sum(Order::fields().total())))
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

An aggregate cannot appear in `.filter()`; that is an `invalid_statement`
error pointing at `.having()`. Calling `.having()` twice combines the
conditions with AND.

## Sorting and limiting groups

An aggregate is an ordering expression too. Use `.asc()` or `.desc()` on it,
with `.nulls_last()` or `.nulls_first()` where the aggregate can be `None`
(see [Sorting, Limits, and Pagination](./sorting-limits-and-pagination.md)),
and `.limit()` to keep the top groups:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Order {
#     #[key]
#     #[auto]
#     id: u64,
#     customer: String,
#     total: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::sum;

// The ten customers with the most revenue.
let top: Vec<(String, Option<i64>)> = Order::all()
    .group_by(Order::fields().customer())
    .select((Order::fields().customer(), sum(Order::fields().total())))
    .order_by(sum(Order::fields().total()).desc().nulls_last())
    .limit(10)
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

Grouped and aggregating queries page with `.limit()` and `.offset()`;
cursor pagination (`.paginate()`) is not supported for them.

## Grouping by a related record's field

A field path through a `belongs_to` relation reads a field of the related
record (see [Reading a related record's field](./querying-records.md#reading-a-related-records-field)).
It can be a grouping key and an aggregate's argument, so "tickets per
assignee" is one query:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct User {
#     #[key]
#     #[auto]
#     id: u64,
#     name: String,
# }
# #[derive(Debug, toasty::Model)]
# struct Ticket {
#     #[key]
#     #[auto]
#     id: u64,
#     title: String,
#     #[index]
#     assignee_id: Option<u64>,
#     #[belongs_to(key = assignee_id, references = id)]
#     assignee: toasty::Deferred<Option<User>>,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::count_all;

// `None` collects the unassigned tickets.
let per_assignee: Vec<(Option<String>, u64)> = Ticket::all()
    .group_by(Ticket::fields().assignee().name().nullable())
    .select((Ticket::fields().assignee().name().nullable(), count_all()))
    .order_by(count_all().desc())
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

The database joins the related table in the same statement. Grouping keys
and aggregate arguments are fields of the queried model or of a record
reached through `belongs_to` relations; fields of embedded types and paths
through `has_many` or `has_one` relations are not supported there.
