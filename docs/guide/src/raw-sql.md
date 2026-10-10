# Raw SQL

Raw SQL runs backend SQL through Toasty's database handles. Use it when you need
a database feature that Toasty's query builders do not expose.

Raw SQL is available on SQL backends: SQLite, Turso, PostgreSQL, and MySQL.
DynamoDB returns an `unsupported_feature` error.

## Statements

Use `toasty::sql::statement` for SQL that does not return rows. It returns the
number of affected rows.

```rust,ignore
let updated = toasty::sql::statement(
    "UPDATE users SET name = ?1 WHERE id = ?2",
)
.bind("Alice")
.bind(1_i64)
.exec(&mut db)
.await?;

assert_eq!(updated, 1);
```

## Queries

Use `toasty::sql::query` for SQL that returns rows. It returns `Vec<Value>`.
Each row is a `Value::Record` with fields in selected-column order.

```rust,ignore
let rows = toasty::sql::query(
    "SELECT id, name FROM users WHERE active = ?1",
)
.bind(true)
.exec(&mut db)
.await?;

for row in rows {
    let toasty::stmt::Value::Record(row) = row else {
        unreachable!("raw SQL queries return record rows");
    };

    println!("id={:?} name={:?}", row[0], row[1]);
}
```

Raw SQL queries do not hydrate models. They return dynamic values so the SQL can
select any expression, function call, join result, or database-specific value.

## Virtual tables and full-text search

Toasty's models and query builders do not know SQLite virtual tables, but raw
SQL does. Create the table in a [hand-written
migration](schema-management.md#writing-a-migration-by-hand), then query it
with `MATCH`. This reads the ids of the `articles` that match a full-text query
against an FTS5 table `article_search`, whose rows follow `articles` through
triggers:

```rust,ignore
let rows = toasty::sql::query(
    "SELECT rowid FROM article_search WHERE article_search MATCH ?1 ORDER BY rank",
)
.bind("flour AND salt")
.column_types([toasty::stmt::Type::I64])
.exec(&mut db)
.await?;

let ids: Vec<i64> = rows
    .into_iter()
    .map(|row| {
        let toasty::stmt::Value::Record(row) = row else {
            unreachable!("raw SQL queries return record rows");
        };
        let toasty::stmt::Value::I64(id) = row[0] else {
            unreachable!("the column type hint makes rowid an I64");
        };
        id
    })
    .collect();

let articles = Article::filter(Article::fields().id().in_list(ids))
    .exec(&mut db)
    .await?;
```

The ids can then go back into a model query, as above. Toasty does not
rewrite the SQL, so `MATCH`, `rank` and the FTS5 query syntax are SQLite's.

## Placeholders

Toasty does not rewrite placeholders in raw SQL. Use the placeholder syntax
reported by `db.capability().sql_placeholder` for the active backend:

| Backend | `SqlPlaceholder` | Placeholder syntax |
|---|---|---|
| SQLite | `NumberedQuestionMark` | `?1`, `?2`, ... |
| Turso | `NumberedQuestionMark` | `?1`, `?2`, ... |
| PostgreSQL | `DollarNumber` | `$1`, `$2`, ... |
| MySQL | `QuestionMark` | `?`, `?`, ... |

Values are bound in the order you call `.bind(...)`.

```rust,ignore
// PostgreSQL
toasty::sql::query("SELECT name FROM users WHERE id = $1")
    .bind(1_i64)
    .exec(&mut db)
    .await?;

// MySQL
toasty::sql::query("SELECT name FROM users WHERE id = ?")
    .bind(1_i64)
    .exec(&mut db)
    .await?;
```

## Binding values

`.bind(value)` infers a database type from common Toasty values: booleans,
integers, floats, strings, bytes, UUIDs, decimals, date/time values, and
non-empty lists.

Use `.bind_typed(value, db_type)` when the type is ambiguous, such as `NULL` or
an empty list:

```rust,ignore
use toasty::schema::db;

toasty::sql::statement(
    "UPDATE users SET archived_at = ?1 WHERE id = ?2",
)
.bind_typed(toasty::stmt::Value::Null, db::Type::Timestamp(6))
.bind(1_i64)
.exec(&mut db)
.await?;
```

## Decoding query results

By default, `query(...).exec(...)` asks the driver to infer result value types
from database metadata.

Inference is exact for many values, but some database values are ambiguous. For
example, SQLite stores booleans as integers and UUIDs as blobs, so a raw query
without type hints decodes them as `I64` and `Bytes`.

Use `.column_types(...)` to provide Toasty result types for selected columns:

```rust,ignore
use toasty::stmt;

let rows = toasty::sql::query(
    "SELECT id, enabled FROM users WHERE id = ?1",
)
.bind(1_i64)
.column_types([stmt::Type::I64, stmt::Type::Bool])
.exec(&mut db)
.await?;
```

Column type hints affect result decoding only. They do not change which SQL
statement is sent to the database.

## Connections and transactions

Raw SQL uses the same executor interface as Toasty query builders. Pass any
`Db`, `Connection`, or `Transaction` handle to `.exec(...)`.

Use a dedicated connection when multiple raw statements need the same physical
database session, such as temporary tables or session variables:

```rust,ignore
let mut conn = db.connection().await?;

toasty::sql::statement("CREATE TEMP TABLE temp_ids (id INTEGER)")
    .exec(&mut conn)
    .await?;

let rows = toasty::sql::query("SELECT id FROM temp_ids")
    .exec(&mut conn)
    .await?;
```

Use a transaction when raw SQL must commit or roll back with other Toasty
operations:

```rust,ignore
let mut tx = db.transaction().await?;

toasty::sql::statement("UPDATE users SET name = ?1 WHERE id = ?2")
    .bind("Alice")
    .bind(1_i64)
    .exec(&mut tx)
    .await?;

User::filter_by_id(1).delete().exec(&mut tx).await?;

tx.commit().await?;
```

Nested transactions work the same way as they do for query builders: raw SQL
executed through the nested transaction is part of that savepoint.

> **Runnable example:** [`store-operations`] runs transactions, savepoints, batches, query-based updates and deletes, and raw SQL.

[`store-operations`]: https://github.com/tokio-rs/toasty/tree/main/examples/store-operations
