use super::Error;

/// Error when a statement violates a constraint declared on a table.
///
/// Drivers classify constraint violations here instead of returning an opaque
/// driver failure, so callers can tell, for example, a duplicate value from any
/// other failure without matching the database's error message. The violation
/// carries whatever detail the database reports: the kind of constraint, the
/// table and columns involved, and the constraint's name. Each detail is
/// optional, since not every database reports every one.
///
/// Drivers currently report [`ConstraintViolationKind::Unique`] violations.
/// The kind is `#[non_exhaustive]`, so other constraint classes can be added
/// without breaking callers.
///
/// # Examples
///
/// ```
/// use toasty_core::{ConstraintViolation, ConstraintViolationKind, Error};
///
/// let err = Error::constraint_violation(
///     ConstraintViolation::new(
///         ConstraintViolationKind::Unique,
///         "UNIQUE constraint failed: users.email",
///     )
///     .with_table("users")
///     .with_columns(["email"]),
/// );
///
/// assert!(err.is_unique_violation());
///
/// let violation = err.as_constraint_violation().unwrap();
/// assert_eq!(violation.table(), Some("users"));
/// assert_eq!(violation.columns(), ["email"]);
/// assert_eq!(violation.constraint(), None);
/// ```
#[derive(Debug, Clone)]
pub struct ConstraintViolation {
    kind: ConstraintViolationKind,
    message: Box<str>,
    table: Option<Box<str>>,
    columns: Vec<String>,
    constraint: Option<Box<str>>,
}

/// The class of constraint a [`ConstraintViolation`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConstraintViolationKind {
    /// A unique constraint or unique index rejected a duplicate value.
    ///
    /// A primary key collision is reported as a unique violation as well.
    Unique,
}

impl ConstraintViolation {
    /// Creates a constraint violation of the given kind.
    ///
    /// `message` is the database's own description of the failure, and it is
    /// what the error displays. Attach the detail the database reports with
    /// [`with_table`](Self::with_table), [`with_columns`](Self::with_columns)
    /// and [`with_constraint`](Self::with_constraint).
    pub fn new(kind: ConstraintViolationKind, message: impl Into<String>) -> ConstraintViolation {
        ConstraintViolation {
            kind,
            message: message.into().into(),
            table: None,
            columns: Vec::new(),
            constraint: None,
        }
    }

    /// Sets the table whose constraint was violated.
    pub fn with_table(mut self, table: impl Into<String>) -> ConstraintViolation {
        self.table = Some(table.into().into());
        self
    }

    /// Sets the columns the violated constraint covers, in the order the
    /// database reports them.
    pub fn with_columns<I>(mut self, columns: I) -> ConstraintViolation
    where
        I: IntoIterator,
        I::Item: Into<String>,
    {
        self.columns = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Sets the name of the violated constraint or index.
    pub fn with_constraint(mut self, constraint: impl Into<String>) -> ConstraintViolation {
        self.constraint = Some(constraint.into().into());
        self
    }

    /// Returns the class of constraint that was violated.
    pub fn kind(&self) -> ConstraintViolationKind {
        self.kind
    }

    /// Returns `true` if a unique constraint or primary key was violated.
    pub fn is_unique(&self) -> bool {
        self.kind == ConstraintViolationKind::Unique
    }

    /// Returns the database's own description of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the table whose constraint was violated, if the database
    /// reported it.
    pub fn table(&self) -> Option<&str> {
        self.table.as_deref()
    }

    /// Returns the columns the violated constraint covers, in the order the
    /// database reported them. Empty if the database did not report them.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Returns the name of the violated constraint or index, if the database
    /// reported it.
    pub fn constraint(&self) -> Option<&str> {
        self.constraint.as_deref()
    }
}

impl std::error::Error for ConstraintViolation {}

impl core::fmt::Display for ConstraintViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "{} constraint violation: {}", self.kind, self.message)
    }
}

impl core::fmt::Display for ConstraintViolationKind {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            ConstraintViolationKind::Unique => f.write_str("unique"),
        }
    }
}

impl Error {
    /// Creates a constraint violation error.
    ///
    /// Drivers return this when the database rejects a statement because it
    /// violates a constraint declared on a table, e.g. SQLite's
    /// `SQLITE_CONSTRAINT_UNIQUE` or PostgreSQL SQLSTATE `23505`.
    ///
    /// # Examples
    ///
    /// ```
    /// use toasty_core::{ConstraintViolation, ConstraintViolationKind, Error};
    ///
    /// let err = Error::constraint_violation(ConstraintViolation::new(
    ///     ConstraintViolationKind::Unique,
    ///     "duplicate key value violates unique constraint \"users_email_key\"",
    /// ));
    /// assert!(err.is_constraint_violation());
    /// assert!(err.is_unique_violation());
    /// ```
    pub fn constraint_violation(violation: ConstraintViolation) -> Error {
        Error::from(super::ErrorKind::ConstraintViolation(violation))
    }

    /// Returns `true` if this error is a constraint violation of any kind.
    pub fn is_constraint_violation(&self) -> bool {
        matches!(self.kind(), super::ErrorKind::ConstraintViolation(_))
    }

    /// Returns `true` if this error is a unique constraint or primary key
    /// violation.
    pub fn is_unique_violation(&self) -> bool {
        self.as_constraint_violation()
            .is_some_and(ConstraintViolation::is_unique)
    }

    /// Returns the violation's detail if this error is a constraint violation.
    pub fn as_constraint_violation(&self) -> Option<&ConstraintViolation> {
        match self.kind() {
            super::ErrorKind::ConstraintViolation(violation) => Some(violation),
            _ => None,
        }
    }
}
