use crate::{Db, Dialect, Error, Executor, Result, stmt::Value};

/// The schema a database has, read back from the database itself.
///
/// [`DatabaseSchema::read`] asks the database for its tables, their columns
/// and their indices. Compare the result with the schema a snapshot records
/// using [`SchemaDrift::between`](super::SchemaDrift::between), for example
/// to check that a migration produced the schema its snapshot claims.
///
/// Only what every SQL database can report is read: names, nullability,
/// uniqueness, the indexed columns and a partial index's predicate. Storage
/// types are driver-specific and are not compared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseSchema {
    /// The tables, ordered by name.
    pub tables: Vec<DatabaseTable>,
}

/// A table of a [`DatabaseSchema`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseTable {
    /// The table name.
    pub name: String,

    /// The columns, in declaration order.
    pub columns: Vec<DatabaseColumn>,

    /// The indices the database reports for the table, ordered by name.
    ///
    /// Indices the database creates on its own for a `PRIMARY KEY` or
    /// `UNIQUE` constraint are not included.
    pub indices: Vec<DatabaseIndex>,
}

/// A column of a [`DatabaseTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseColumn {
    /// The column name.
    pub name: String,

    /// The type the column was declared with, as the database reports it.
    pub declared_type: String,

    /// `true` unless the column was declared `NOT NULL`.
    pub nullable: bool,

    /// `true` when the column is part of the table's primary key.
    pub primary_key: bool,
}

/// An index of a [`DatabaseTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseIndex {
    /// The index name.
    pub name: String,

    /// `true` for a unique index.
    pub unique: bool,

    /// The indexed columns, in index order. `None` stands for an indexed
    /// expression.
    pub columns: Vec<Option<String>>,

    /// A partial index's predicate, the text after `WHERE` in the statement
    /// that created it, as the database recorded it. `None` for an index over
    /// every row.
    pub filter: Option<String>,
}

impl DatabaseSchema {
    /// Reads the schema of the database `db` is connected to.
    ///
    /// Toasty's own migration tracking table and the database's internal
    /// tables are left out.
    ///
    /// # Errors
    ///
    /// Returns an `unsupported_feature` error for a driver that cannot report
    /// its schema yet. SQLite can.
    ///
    /// # Examples
    ///
    /// ```
    /// # #[cfg(feature = "sqlite")]
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// use toasty::migration::DatabaseSchema;
    ///
    /// # #[derive(Debug, toasty::Model)]
    /// # struct User {
    /// #     #[key]
    /// #     id: i64,
    /// #     #[unique]
    /// #     email: String,
    /// # }
    /// let db = toasty::Db::builder()
    ///     .models(toasty::models!(User))
    ///     .connect("sqlite::memory:")
    ///     .await
    ///     .unwrap();
    /// db.push_schema().await.unwrap();
    ///
    /// let schema = DatabaseSchema::read(&db).await.unwrap();
    /// let users = schema.table("users").unwrap();
    /// assert!(users.indices.iter().any(|index| index.unique));
    /// # });
    /// ```
    pub async fn read(db: &Db) -> Result<Self> {
        match db.capability().sql {
            Some(Dialect::Sqlite) => {
                let mut connection = db.connection().await?;
                read_sqlite(&mut connection).await
            }
            _ => Err(Error::unsupported_feature(format!(
                "reading the database schema is not supported by the {} driver",
                db.capability().driver_name
            ))),
        }
    }

    /// Returns the table named `name`.
    pub fn table(&self, name: &str) -> Option<&DatabaseTable> {
        self.tables.iter().find(|table| table.name == name)
    }
}

impl DatabaseTable {
    /// Returns the column named `name`.
    pub fn column(&self, name: &str) -> Option<&DatabaseColumn> {
        self.columns.iter().find(|column| column.name == name)
    }

    /// Returns the index named `name`.
    pub fn index(&self, name: &str) -> Option<&DatabaseIndex> {
        self.indices.iter().find(|index| index.name == name)
    }
}

/// The name of the table the drivers record applied migrations in.
const MIGRATIONS_TABLE: &str = "__toasty_migrations";

async fn read_sqlite(executor: &mut dyn Executor) -> Result<DatabaseSchema> {
    let names = rows(
        executor,
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
        &[],
    )
    .await?;

    let mut tables = Vec::with_capacity(names.len());
    for row in names {
        let name = text(&row, 0)?;
        if name == MIGRATIONS_TABLE {
            continue;
        }

        let mut columns = Vec::new();
        for row in rows(
            executor,
            "SELECT name, type, \"notnull\", pk FROM pragma_table_info(?1) ORDER BY cid",
            &[&name],
        )
        .await?
        {
            columns.push(DatabaseColumn {
                name: text(&row, 0)?,
                declared_type: text(&row, 1)?,
                nullable: integer(&row, 2)? == 0,
                primary_key: integer(&row, 3)? != 0,
            });
        }

        let mut indices = Vec::new();
        for row in rows(
            executor,
            "SELECT name, \"unique\", partial FROM pragma_index_list(?1) ORDER BY name",
            &[&name],
        )
        .await?
        {
            let index = text(&row, 0)?;
            // `PRIMARY KEY` and `UNIQUE` constraints get an index SQLite
            // names and manages itself; no migration creates or drops it.
            if index.starts_with("sqlite_autoindex_") {
                continue;
            }
            let mut indexed = Vec::new();
            for row in rows(
                executor,
                "SELECT name FROM pragma_index_info(?1) ORDER BY seqno",
                &[&index],
            )
            .await?
            {
                indexed.push(match field(&row, 0)? {
                    Value::Null => None,
                    _ => Some(text(&row, 0)?),
                });
            }
            // SQLite keeps no parsed form of a partial index's predicate,
            // only the `CREATE INDEX` statement as it was written.
            let filter = if integer(&row, 2)? != 0 {
                let created = rows(
                    executor,
                    "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    &[&index],
                )
                .await?;
                let sql = created
                    .first()
                    .map(|row| text(row, 0))
                    .transpose()?
                    .unwrap_or_default();
                Some(index_predicate(&sql).ok_or_else(|| {
                    Error::invalid_result(format!(
                        "partial index {index:?} has no WHERE clause in {sql:?}"
                    ))
                })?)
            } else {
                None
            };
            indices.push(DatabaseIndex {
                name: index,
                unique: integer(&row, 1)? != 0,
                columns: indexed,
                filter,
            });
        }

        tables.push(DatabaseTable {
            name,
            columns,
            indices,
        });
    }

    Ok(DatabaseSchema { tables })
}

/// The predicate of a `CREATE INDEX ... WHERE <predicate>` statement: the
/// text after the first `WHERE` keyword outside quotes and parentheses,
/// trimmed. `None` when the statement has no such `WHERE`.
fn index_predicate(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                // A doubled quote inside a quoted span is an escaped quote,
                // so skipping to each closing quote in turn is enough.
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
            }
            b'[' => {
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
            }
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            _ if depth == 0
                && sql[i..]
                    .get(..5)
                    .is_some_and(|word| word.eq_ignore_ascii_case("where"))
                && (i == 0 || !is_identifier_byte(bytes[i - 1]))
                && bytes.get(i + 5).is_none_or(|b| !is_identifier_byte(*b)) =>
            {
                let predicate = sql[i + 5..].trim().trim_end_matches(';').trim_end();
                return Some(predicate.to_string());
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

async fn rows(executor: &mut dyn Executor, sql: &str, params: &[&str]) -> Result<Vec<Value>> {
    let mut query = crate::sql::query(sql);
    for param in params {
        query = query.bind(*param);
    }
    query.exec(executor).await
}

fn field(row: &Value, index: usize) -> Result<&Value> {
    let Value::Record(record) = row else {
        return Err(Error::invalid_result("schema query row is not a record"));
    };
    record
        .get(index)
        .ok_or_else(|| Error::invalid_result("schema query row has too few columns"))
}

fn text(row: &Value, index: usize) -> Result<String> {
    match field(row, index)? {
        Value::String(value) => Ok(value.clone()),
        // A column declared without a type reports an empty one.
        Value::Null => Ok(String::new()),
        value => Err(Error::invalid_result(format!(
            "schema query returned {value:?} where text was expected"
        ))),
    }
}

fn integer(row: &Value, index: usize) -> Result<i64> {
    match field(row, index)? {
        Value::I64(value) => Ok(*value),
        value => Err(Error::invalid_result(format!(
            "schema query returned {value:?} where an integer was expected"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::index_predicate;

    #[test]
    fn index_predicate_is_the_text_after_where() {
        assert_eq!(
            index_predicate(
                r#"CREATE UNIQUE INDEX "idx" ON "tokens" ("owner", "name") WHERE revoked_at IS NULL"#
            )
            .as_deref(),
            Some("revoked_at IS NULL"),
        );
    }

    #[test]
    fn index_predicate_ignores_where_inside_quotes_and_parentheses() {
        assert_eq!(
            index_predicate(
                r#"CREATE INDEX "where" ON "t" ("where", "a_where") WHERE "where" = 'a where b' AND (x > 1)"#
            )
            .as_deref(),
            Some(r#""where" = 'a where b' AND (x > 1)"#),
        );
    }

    #[test]
    fn index_predicate_is_none_without_where() {
        assert_eq!(
            index_predicate(r#"CREATE INDEX "nowhere" ON "t" ("somewhere")"#),
            None
        );
    }
}
