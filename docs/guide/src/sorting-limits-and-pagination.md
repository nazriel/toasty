# Sorting, Limits, and Pagination

Queries return results in an unspecified order by default. Use `.order_by()` to
sort results, `.limit()` and `.offset()` to restrict the result set, and
`.paginate()` to walk through results one page at a time.

## Sorting with `order_by`

Call `.order_by()` on a query with a field path and a direction — `.asc()` for
ascending or `.desc()` for descending:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Ascending: smallest order first
let items = Item::all()
    .order_by(Item::fields().order().asc())
    .exec(&mut db)
    .await?;

// Descending: largest order first
let items = Item::all()
    .order_by(Item::fields().order().desc())
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

`Item::fields().order()` returns a field path. Calling `.asc()` or `.desc()` on
it produces an ordering expression that `.order_by()` accepts.

### Sorting by multiple fields

Pass a tuple of ordering expressions to sort by several fields at once. Each
field beyond the first acts as a tie-breaker for the ones before it:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct User {
#     #[key]
#     #[auto]
#     id: u64,
#     name: String,
#     age: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Sort by age ascending; users with the same age are ordered by name descending
let users = User::all()
    .order_by((
        User::fields().age().asc(),
        User::fields().name().desc(),
    ))
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

Chained `.order_by()` calls behave the same way — each call appends its
expressions to the existing order rather than replacing them, so the two forms
below are equivalent:

```rust,ignore
q.order_by((User::fields().age().asc(), User::fields().name().desc()));

q.order_by(User::fields().age().asc())
 .order_by(User::fields().name().desc());
```

Sorting works with filters too:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
#     category: String,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
let items = Item::filter(Item::fields().category().eq("books"))
    .order_by(Item::fields().order().desc())
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

### Placing `NULL` values

Where `NULL` values of a nullable field land depends on the database:
SQLite and MySQL sort them before every other value, PostgreSQL after them,
and a descending order reverses either. Call `.nulls_last()` or
`.nulls_first()` on an ordering expression to choose, in either direction and
on every SQL database:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Task {
#     #[key]
#     #[auto]
#     id: u64,
#     title: String,
#     priority: Option<i64>,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Highest priority first; tasks without a priority at the end
let tasks = Task::all()
    .order_by(Task::fields().priority().desc().nulls_last())
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

SQLite and PostgreSQL write it as `NULLS LAST` / `NULLS FIRST`. MySQL and
MariaDB have no such clause, so Toasty adds a leading `<field> IS NULL` (or
`IS NOT NULL`) key to the ordering, which produces the same order. Cursor
pagination follows the chosen placement. DynamoDB orders by its sort key alone
and rejects a placement with an unsupported-feature error.

### Sorting by a value of a related record

A value that lives in another table, such as a user's setting in a table of
settings, can order a query. `Query::value_for` turns a query of the other
model into that value: the field of the record whose key matches the record
being sorted, or `None` when there is none. Order by it with `.asc()` or
`.desc()`, like a field:

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
# struct Setting {
#     #[key]
#     #[auto]
#     id: u64,
#     user_id: u64,
#     key: String,
#     value: String,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::{List, Query};

// Each user's "theme" setting
let theme = Query::<List<Setting>>::all()
    .filter(Setting::fields().key().eq("theme"))
    .value_for(
        Setting::fields().value(),   // the value to sort by
        Setting::fields().user_id(), // the setting's reference to its user...
        User::fields().id(),         // ...matching the sorted user's key
    );

// Users by theme; users without one at the end, then by name
let users = User::all()
    .order_by((theme.asc().nulls_last(), User::fields().name().asc()))
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

The value is a correlated subquery that the database evaluates for each row:

```sql
SELECT ... FROM users
ORDER BY (SELECT value FROM settings
          WHERE key = $1 AND user_id = users.id) ASC NULLS LAST,
         name ASC
```

Every value in the filter is a bound parameter, so which value to sort by can
be chosen at run time. The filter must select at most one record for each
sorted record: when it selects more, SQLite uses the first it finds, while
PostgreSQL and MySQL fail the query. Combine it with `.nulls_last()` or
`.nulls_first()`, since a record without a match sorts as `NULL`.

Sorting by a related value works on SQL databases with `.limit()` and
`.offset()`. It is not available with cursor pagination, which has no column
to read the next cursor from, nor in a filter, nor on DynamoDB; each of these
is rejected with an unsupported-feature error.

### Reading a value of a related record

The same value can be read with `.select(...)`, next to the record's own
fields, as an `Option`: `None` for a record without a match. A relation
predicate such as `.any(...)` reads as a `bool` the same way. Both are
subqueries in the statement's `SELECT` list, so the query is still one
statement:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct User {
#     #[key]
#     #[auto]
#     id: u64,
#     name: String,
#     #[has_many]
#     settings: toasty::Deferred<Vec<Setting>>,
# }
# #[derive(Debug, toasty::Model)]
# struct Setting {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     user_id: u64,
#     #[belongs_to(key = user_id, references = id)]
#     user: toasty::Deferred<User>,
#     key: String,
#     value: String,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
use toasty::stmt::{List, Query};

let theme = Query::<List<Setting>>::all()
    .filter(Setting::fields().key().eq("theme"))
    .value_for(
        Setting::fields().value(),
        Setting::fields().user_id(),
        User::fields().id(),
    );
let has_settings = User::fields()
    .settings()
    .any(Setting::fields().key().ne(""));

// Each user's name, theme and whether they have any setting
let rows: Vec<(String, Option<String>, bool)> = User::all()
    .select((User::fields().name(), theme, has_settings))
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

```sql
SELECT name,
       (SELECT value FROM settings WHERE user_id = users.id AND key = $1),
       id IN (SELECT user_id FROM settings WHERE key <> $2)
FROM users
```

A selected value must be a non-nullable field that is not an embedded type:
a nullable field would read as `Option<Option<T>>`, and an embedded type
spans several columns; both are rejected with an unsupported-feature error.
In a query with `.group_by(...)`, a related value is read when the field it
matches (`User::fields().id()` above) is a grouping key, and `.any(...)` is
not read. Selecting either is SQL-only; DynamoDB rejects it with an
unsupported-feature error.

## Limiting results

`.limit(n)` caps the number of records returned:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// At most 5 items
let items = Item::all().limit(5).exec(&mut db).await?;
# Ok(())
# }
```

If the query matches fewer records than the limit, all matching records are
returned.

`.limit(n)` is an upper bound, not a guarantee. Toasty applies the limit to
the database query, but it may filter the returned rows further before
producing the final result set. When that happens, a query can return fewer
than `n` records even if more than `n` rows match the filter expression. Use
[cursor-based pagination](#cursor-based-pagination) to walk every matching
record.

Combine `.order_by()` with `.limit()` to get the top or bottom N records:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Top 7 items by order (highest first)
let items = Item::all()
    .order_by(Item::fields().order().desc())
    .limit(7)
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

## Offset

`.offset(n)` skips the first `n` results. It requires `.limit()` to be called
first:

```rust
# use toasty::Model;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Skip the first 5, then return the next 7
let items = Item::all()
    .order_by(Item::fields().order().asc())
    .limit(7)
    .offset(5)
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

Limit and offset work for simple cases, but cursor-based pagination (below) is a
better fit for paging through large result sets. Offset-based pagination gets
slower as the offset increases because the database still reads and discards the
skipped rows. It can also produce inconsistent results when rows are inserted or
deleted between page fetches. See Markus Winand's
["No Offset"](https://use-the-index-luke.com/no-offset) for an in-depth
explanation.

## Cursor-based pagination

`.paginate(per_page)` splits results into pages. It requires `.order_by()` and
returns a `Page` instead of a `Vec`:

```rust
# use toasty::Model;
# use toasty::stmt::Page;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
let page: Page<_> = Item::all()
    .order_by(Item::fields().order().desc())
    .paginate(10)
    .exec(&mut db)
    .await?;

// Access items in the page
for item in page.iter() {
    println!("order: {}", item.order);
}

// Check how many items are in this page
println!("items: {}", page.len());
# Ok(())
# }
```

A `Page` dereferences to a slice, so you can index into it, iterate over it, and
call slice methods like `.len()` and `.iter()` directly.

`per_page` is an upper bound on the page size, not a guarantee. Toasty
applies it to the database query, but it may filter the returned rows further
before producing the page. A page can therefore contain fewer than `per_page`
items even when more results exist. Check `.has_next()` (or follow
`.next()` until it returns `None`) to detect the end of the result set rather
than relying on the size of any individual page.

### Navigating pages

`Page` provides `.next()` and `.prev()` methods that fetch the next or previous
page. Both return `Option<Page>` — `None` when there are no more results in that
direction:

```rust
# use toasty::Model;
# use toasty::stmt::Page;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
let first_page: Page<_> = Item::all()
    .order_by(Item::fields().order().asc())
    .paginate(10)
    .exec(&mut db)
    .await?;

// Move to the next page
if let Some(second_page) = first_page.next(&mut db).await? {
    println!("page 2 has {} items", second_page.len());

    // Go back
    if let Some(back) = second_page.prev(&mut db).await? {
        println!("back to page 1: {} items", back.len());
    }
}
# Ok(())
# }
```

Use `.has_next()` and `.has_prev()` to check whether more pages exist without
fetching them:

```rust,ignore
if page.has_next() {
    let next = page.next(&mut db).await?.unwrap();
}
```

### Walking all pages

To process every record in pages:

```rust
# use toasty::Model;
# use toasty::stmt::Page;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
let mut page: Page<_> = Item::all()
    .order_by(Item::fields().order().asc())
    .paginate(10)
    .exec(&mut db)
    .await?;

loop {
    for item in page.iter() {
        println!("order: {}", item.order);
    }

    match page.next(&mut db).await? {
        Some(next) => page = next,
        None => break,
    }
}
# Ok(())
# }
```

### Starting from a cursor position

Use `.after()` to start pagination after a specific value in the sort field:

```rust
# use toasty::Model;
# use toasty::stmt::Page;
# #[derive(Debug, toasty::Model)]
# struct Item {
#     #[key]
#     #[auto]
#     id: u64,
#     #[index]
#     order: i64,
# }
# async fn __example(mut db: toasty::Db) -> toasty::Result<()> {
// Start after order=90 (descending), so the first item will be order=89
let page: Page<_> = Item::all()
    .order_by(Item::fields().order().desc())
    .paginate(10)
    .after(90)
    .exec(&mut db)
    .await?;
# Ok(())
# }
```

The value passed to `.after()` corresponds to the field used in `.order_by()`.

Toasty adds primary-key ordering fields when the requested ordering does not
uniquely identify each row. Cursors returned by `Page` include those fields, so
`.next()` and `.prev()` preserve a deterministic order when multiple rows have
the same requested ordering values.

A cursor supplied directly to `.after()` or `.before()` may omit the internal
primary-key fields. Toasty accepts the cursor as a prefix of the complete
ordering. A prefix cursor cannot identify a position within rows that have the
same prefix values. After the initial query, use the cursor returned by `Page`
to continue without skipping tied rows.

## Method summary

Methods available on query builders:

| Method | Description |
|---|---|
| `.order_by(field.asc())` | Sort ascending by field |
| `.order_by(field.desc())` | Sort descending by field |
| `.order_by((a.asc(), b.desc()))` | Sort by multiple fields; later fields are tie-breakers |
| `.order_by(field.asc().nulls_last())` | Sort with `NULL` values last (or `.nulls_first()`) |
| `.order_by(query.value_for(field, key, outer).asc())` | Sort by a value of a related record (SQL, offset pages) |
| `.limit(n)` | Return at most `n` records |
| `.offset(n)` | Skip first `n` records (requires `.limit()`) |
| `.paginate(per_page)` | Cursor-based pagination (requires `.order_by()`) |

Methods available on `Page`:

| Method | Returns | Description |
|---|---|---|
| `.next(&mut db)` | `Result<Option<Page>>` | Fetch next page |
| `.prev(&mut db)` | `Result<Option<Page>>` | Fetch previous page |
| `.has_next()` | `bool` | Whether a next page exists |
| `.has_prev()` | `bool` | Whether a previous page exists |
| `.items` | `Vec<M>` | The records in this page |
| `.len()` | `usize` | Number of items (via `Deref` to slice) |
| `.iter()` | iterator | Iterate items (via `Deref` to slice) |

> **Runnable example:** [`product-search`] builds filter expressions, sorts, cursor-paginates, and projects columns.

[`product-search`]: https://github.com/tokio-rs/toasty/tree/main/examples/product-search
