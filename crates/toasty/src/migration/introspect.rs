use crate::{Db, Dialect, Error, Executor, Result, stmt::Value};

/// The schema a database has, read back from the database itself.
///
/// [`DatabaseSchema::read`] asks the database for its tables, their columns
/// and their indices. Compare the result with the schema a snapshot records
/// using [`SchemaDrift::between`](super::SchemaDrift::between), for example
/// to check that a migration produced the schema its snapshot claims.
///
/// Only what every SQL database can report is read: names, nullability,
/// uniqueness and the indexed columns. Storage types are driver-specific
/// and are not compared.
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
            "SELECT name, \"unique\" FROM pragma_index_list(?1) ORDER BY name",
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
            indices.push(DatabaseIndex {
                name: index,
                unique: integer(&row, 1)? != 0,
                columns: indexed,
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
