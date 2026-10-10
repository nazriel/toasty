/// A database migration generated from a [`diff::Schema`](super::super::diff::Schema) by a driver.
///
/// Currently only SQL migrations are supported. Multiple SQL statements
/// within a single migration are separated by breakpoint markers
/// (`-- #[toasty::breakpoint]`).
///
/// # Examples
///
/// ```ignore
/// use toasty_core::schema::db::Migration;
///
/// let m = Migration::new_sql("CREATE TABLE users (id INTEGER PRIMARY KEY)".to_string());
/// assert_eq!(m.statements(), vec!["CREATE TABLE users (id INTEGER PRIMARY KEY)"]);
/// ```
#[derive(Debug)]
pub enum Migration {
    /// A SQL migration containing one or more statements.
    Sql(String),
}

impl Migration {
    /// Creates a SQL migration from a single SQL string.
    pub fn new_sql(sql: String) -> Self {
        Migration::Sql(sql)
    }

    /// Creates a SQL migration from multiple SQL statements.
    /// Statements are joined with `-- #[toasty::breakpoint]` markers.
    pub fn new_sql_with_breakpoints<S: AsRef<str>>(statements: &[S]) -> Self {
        let sql = statements
            .iter()
            .map(|s| s.as_ref())
            .collect::<Vec<_>>()
            .join("\n-- #[toasty::breakpoint]\n");
        Migration::Sql(sql)
    }

    /// Returns individual SQL statements by splitting on breakpoint markers.
    ///
    /// A chunk that holds no SQL, only whitespace and `--` comment lines, is
    /// not a statement and is left out: a driver would send it to the
    /// database as an empty query, which some databases reject. A chunk that
    /// holds SQL together with comments is returned as it is.
    ///
    /// Only whole lines starting with `--` count as comments. A `/* */` block
    /// comment, or a `--` that continues a multi-line string literal, makes
    /// the chunk count as SQL.
    pub fn statements(&self) -> Vec<&str> {
        match self {
            Migration::Sql(sql) => sql
                .split("\n-- #[toasty::breakpoint]\n")
                .filter(|chunk| has_sql(chunk))
                .collect(),
        }
    }
}

/// Returns `true` if `chunk` has a line that is neither blank nor a `--`
/// comment.
fn has_sql(chunk: &str) -> bool {
    chunk.lines().any(|line| {
        let line = line.trim_start();
        !line.is_empty() && !line.starts_with("--")
    })
}

/// Metadata about a migration that has already been applied to a database.
///
/// Stores the unique migration ID assigned by the migration system.
///
/// # Examples
///
/// ```ignore
/// use toasty_core::schema::db::AppliedMigration;
///
/// let applied = AppliedMigration::new(42);
/// assert_eq!(applied.id(), 42);
/// ```
pub struct AppliedMigration {
    id: u64,
}

impl AppliedMigration {
    /// Creates a new `AppliedMigration` with the given ID.
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    /// Returns the migration's unique ID.
    pub fn id(&self) -> u64 {
        self.id
    }
}
