use std::fmt;

use crate::schema::db;

use super::DatabaseSchema;

/// The differences between the schema a snapshot records and the schema a
/// database has.
///
/// An empty drift means the database matches. Build one with
/// [`SchemaDrift::between`]; its [`Display`](fmt::Display) lists one
/// difference per line.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "sqlite")]
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// use toasty::migration::{DatabaseSchema, SchemaDrift};
///
/// # #[derive(Debug, toasty::Model)]
/// # struct User {
/// #     #[key]
/// #     id: i64,
/// #     #[unique]
/// #     email: String,
/// # }
/// let mut db = toasty::Db::builder()
///     .models(toasty::models!(User))
///     .connect("sqlite::memory:")
///     .await
///     .unwrap();
/// db.push_schema().await.unwrap();
///
/// let actual = DatabaseSchema::read(&db).await.unwrap();
/// assert!(SchemaDrift::between(&db.schema().db, &actual).is_empty());
///
/// toasty::sql::statement("DROP INDEX index_users_by_email")
///     .exec(&mut db)
///     .await
///     .unwrap();
/// let actual = DatabaseSchema::read(&db).await.unwrap();
/// let drift = SchemaDrift::between(&db.schema().db, &actual);
/// assert_eq!(
///     drift.to_string(),
///     "index \"users\".\"index_users_by_email\" is missing (unique on \"email\")\n",
/// );
/// # });
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaDrift {
    /// Every difference found, ordered by table.
    pub differences: Vec<SchemaDifference>,
}

/// One difference of a [`SchemaDrift`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaDifference {
    /// The expected schema has a table the database lacks.
    MissingTable {
        /// The table name.
        table: String,
    },

    /// The database has a table the expected schema does not record.
    UnexpectedTable {
        /// The table name.
        table: String,
    },

    /// The expected schema has a column the database lacks.
    MissingColumn {
        /// The table name.
        table: String,
        /// The column name.
        column: String,
    },

    /// The database has a column the expected schema does not record.
    UnexpectedColumn {
        /// The table name.
        table: String,
        /// The column name.
        column: String,
    },

    /// A column is nullable on one side and `NOT NULL` on the other.
    ChangedNullability {
        /// The table name.
        table: String,
        /// The column name.
        column: String,
        /// Whether the expected schema allows `NULL`.
        expected: bool,
        /// Whether the database allows `NULL`.
        actual: bool,
    },

    /// The expected schema has an index the database lacks.
    MissingIndex {
        /// The table name.
        table: String,
        /// The index the expected schema records.
        index: IndexShape,
    },

    /// The database has an index the expected schema does not record.
    UnexpectedIndex {
        /// The table name.
        table: String,
        /// The index the database has.
        index: IndexShape,
    },

    /// An index exists on both sides with a different uniqueness or columns.
    ChangedIndex {
        /// The table name.
        table: String,
        /// The index the expected schema records.
        expected: IndexShape,
        /// The index the database has.
        actual: IndexShape,
    },
}

/// What [`SchemaDrift`] compares of an index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexShape {
    /// The index name.
    pub name: String,
    /// `true` for a unique index.
    pub unique: bool,
    /// The indexed columns, in index order. `None` stands for an indexed
    /// expression.
    pub columns: Vec<Option<String>>,
}

impl SchemaDrift {
    /// Compares the schema `expected` records, typically a migration
    /// snapshot's, with the schema `actual` a database reported.
    ///
    /// Tables, columns and indices are matched by name. A column is compared
    /// by its nullability; an index by its uniqueness and its columns, in
    /// order. A primary key's index is not required: SQL drivers create the
    /// key as part of the table, and the migration generator emits no
    /// statement for it. When the database does have an index by that name,
    /// it is compared like any other.
    pub fn between(expected: &db::Schema, actual: &DatabaseSchema) -> Self {
        let mut differences = Vec::new();

        for table in &expected.tables {
            let Some(found) = actual.table(&table.name) else {
                differences.push(SchemaDifference::MissingTable {
                    table: table.name.clone(),
                });
                continue;
            };

            for column in &table.columns {
                match found.column(&column.name) {
                    None => differences.push(SchemaDifference::MissingColumn {
                        table: table.name.clone(),
                        column: column.name.clone(),
                    }),
                    Some(present) if present.nullable != column.nullable => {
                        differences.push(SchemaDifference::ChangedNullability {
                            table: table.name.clone(),
                            column: column.name.clone(),
                            expected: column.nullable,
                            actual: present.nullable,
                        })
                    }
                    Some(_) => {}
                }
            }
            for column in &found.columns {
                if !table.columns.iter().any(|c| c.name == column.name) {
                    differences.push(SchemaDifference::UnexpectedColumn {
                        table: table.name.clone(),
                        column: column.name.clone(),
                    });
                }
            }

            for declared in &table.indices {
                let index = IndexShape {
                    name: declared.name.clone(),
                    unique: declared.unique,
                    columns: declared
                        .columns
                        .iter()
                        .map(|column| Some(expected.column(column.column).name.clone()))
                        .collect(),
                };
                match found.index(&index.name) {
                    None if declared.primary_key => {}
                    None => differences.push(SchemaDifference::MissingIndex {
                        table: table.name.clone(),
                        index,
                    }),
                    Some(present) => {
                        let present = IndexShape::from(present);
                        if present != index {
                            differences.push(SchemaDifference::ChangedIndex {
                                table: table.name.clone(),
                                expected: index,
                                actual: present,
                            });
                        }
                    }
                }
            }
            for index in &found.indices {
                if !table
                    .indices
                    .iter()
                    .any(|declared| declared.name == index.name)
                {
                    differences.push(SchemaDifference::UnexpectedIndex {
                        table: table.name.clone(),
                        index: index.into(),
                    });
                }
            }
        }

        for table in &actual.tables {
            if !expected.tables.iter().any(|t| t.name == table.name) {
                differences.push(SchemaDifference::UnexpectedTable {
                    table: table.name.clone(),
                });
            }
        }

        Self { differences }
    }

    /// Returns `true` when the database matches the expected schema.
    pub fn is_empty(&self) -> bool {
        self.differences.is_empty()
    }
}

impl From<&super::DatabaseIndex> for IndexShape {
    fn from(index: &super::DatabaseIndex) -> Self {
        Self {
            name: index.name.clone(),
            unique: index.unique,
            columns: index.columns.clone(),
        }
    }
}

impl fmt::Display for SchemaDrift {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for difference in &self.differences {
            writeln!(f, "{difference}")?;
        }
        Ok(())
    }
}

impl fmt::Display for SchemaDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTable { table } => write!(f, "table {table:?} is missing"),
            Self::UnexpectedTable { table } => {
                write!(f, "table {table:?} is not in the expected schema")
            }
            Self::MissingColumn { table, column } => {
                write!(f, "column {table:?}.{column:?} is missing")
            }
            Self::UnexpectedColumn { table, column } => {
                write!(
                    f,
                    "column {table:?}.{column:?} is not in the expected schema"
                )
            }
            Self::ChangedNullability {
                table,
                column,
                expected,
                actual,
            } => write!(
                f,
                "column {table:?}.{column:?} is {} but expected {}",
                nullability(*actual),
                nullability(*expected),
            ),
            Self::MissingIndex { table, index } => {
                write!(f, "index {table:?}.{:?} is missing ({index})", index.name)
            }
            Self::UnexpectedIndex { table, index } => write!(
                f,
                "index {table:?}.{:?} is not in the expected schema ({index})",
                index.name
            ),
            Self::ChangedIndex {
                table,
                expected,
                actual,
            } => write!(
                f,
                "index {table:?}.{:?} is {actual} but expected {expected}",
                expected.name
            ),
        }
    }
}

impl fmt::Display for IndexShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.unique { "unique on " } else { "on " })?;
        for (i, column) in self.columns.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            match column {
                Some(name) => write!(f, "{name:?}")?,
                None => f.write_str("an expression")?,
            }
        }
        Ok(())
    }
}

fn nullability(nullable: bool) -> &'static str {
    if nullable { "nullable" } else { "NOT NULL" }
}
