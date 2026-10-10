use rusqlite::ffi;
use toasty_core::{ConstraintViolation, ConstraintViolationKind};

/// Converts a rusqlite error into a Toasty error.
///
/// A violation is recognised by SQLite's extended result code (rusqlite turns
/// extended result codes on for every connection it opens), never by the
/// message: `SQLITE_CONSTRAINT_UNIQUE` and `SQLITE_CONSTRAINT_PRIMARYKEY` are
/// unique violations. The table and columns come from the message, which is
/// the only place SQLite reports them; if it does not have the expected
/// shape, the error is still a unique violation, without that detail.
/// Everything else is a driver operation failure.
pub(crate) fn classify_sqlite_error(err: rusqlite::Error) -> toasty_core::Error {
    if let rusqlite::Error::SqliteFailure(failure, message) = &err
        && matches!(
            failure.extended_code,
            ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    {
        let message = message.clone().unwrap_or_else(|| failure.to_string());
        return toasty_core::Error::constraint_violation(unique_violation(message));
    }

    toasty_core::Error::driver_operation_failed(err)
}

/// Builds a unique violation from SQLite's message, taking the detail it
/// offers.
///
/// SQLite words a violation of a unique index on columns as
/// `UNIQUE constraint failed: <table>.<column>[, <table>.<column>...]`, and one
/// on an index over expressions as `UNIQUE constraint failed: index '<name>'`.
fn unique_violation(message: String) -> ConstraintViolation {
    let detail = message
        .strip_prefix("UNIQUE constraint failed: ")
        .map(str::to_owned);
    let mut violation = ConstraintViolation::new(ConstraintViolationKind::Unique, message);

    let Some(detail) = detail else {
        return violation;
    };

    if let Some(index) = detail
        .strip_prefix("index '")
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return violation.with_constraint(index);
    }

    if let Some((table, columns)) = table_and_columns(&detail) {
        violation = violation.with_table(table).with_columns(columns);
    }
    violation
}

/// Parses `<table>.<column>[, <table>.<column>...]`. Every column must name
/// the same table.
fn table_and_columns(detail: &str) -> Option<(&str, Vec<&str>)> {
    let mut table = None;
    let mut columns = vec![];

    for qualified in detail.split(", ") {
        let (t, column) = qualified.split_once('.')?;
        if t.is_empty() || column.is_empty() || table.is_some_and(|table| table != t) {
            return None;
        }
        table = Some(t);
        columns.push(column);
    }

    Some((table?, columns))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(extended_code: i32, message: &str) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(ffi::Error::new(extended_code), Some(message.to_owned()))
    }

    #[test]
    fn unique_violation_on_one_column() {
        let err = classify_sqlite_error(failure(
            ffi::SQLITE_CONSTRAINT_UNIQUE,
            "UNIQUE constraint failed: users.email",
        ));
        let violation = err.as_constraint_violation().unwrap();
        assert!(violation.is_unique());
        assert_eq!(violation.table(), Some("users"));
        assert_eq!(violation.columns(), ["email"]);
        assert_eq!(violation.constraint(), None);
        assert_eq!(
            err.to_string(),
            "unique constraint violation: UNIQUE constraint failed: users.email"
        );
    }

    #[test]
    fn unique_violation_on_several_columns() {
        let err = classify_sqlite_error(failure(
            ffi::SQLITE_CONSTRAINT_UNIQUE,
            "UNIQUE constraint failed: members.tenant, members.email",
        ));
        let violation = err.as_constraint_violation().unwrap();
        assert_eq!(violation.table(), Some("members"));
        assert_eq!(violation.columns(), ["tenant", "email"]);
    }

    #[test]
    fn primary_key_violation_is_a_unique_violation() {
        let err = classify_sqlite_error(failure(
            ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
            "UNIQUE constraint failed: users.id",
        ));
        assert!(err.is_unique_violation());
        let violation = err.as_constraint_violation().unwrap();
        assert_eq!(violation.table(), Some("users"));
        assert_eq!(violation.columns(), ["id"]);
    }

    #[test]
    fn unique_violation_on_an_expression_index_reports_the_index_name() {
        let err = classify_sqlite_error(failure(
            ffi::SQLITE_CONSTRAINT_UNIQUE,
            "UNIQUE constraint failed: index 'index_users_by_lower_email'",
        ));
        let violation = err.as_constraint_violation().unwrap();
        assert_eq!(violation.constraint(), Some("index_users_by_lower_email"));
        assert_eq!(violation.table(), None);
        assert!(violation.columns().is_empty());
    }

    #[test]
    fn unique_violation_with_an_unexpected_message_has_no_detail() {
        for message in [
            "something else entirely",
            "UNIQUE constraint failed: ",
            "UNIQUE constraint failed: users.email, accounts.email",
            "UNIQUE constraint failed: email",
        ] {
            let err = classify_sqlite_error(failure(ffi::SQLITE_CONSTRAINT_UNIQUE, message));
            let violation = err.as_constraint_violation().unwrap();
            assert!(violation.is_unique(), "{message}");
            assert_eq!(violation.table(), None, "{message}");
            assert!(violation.columns().is_empty(), "{message}");
            assert_eq!(violation.message(), message);
        }
    }

    #[test]
    fn unique_violation_without_a_message_uses_the_code_description() {
        let err = classify_sqlite_error(rusqlite::Error::SqliteFailure(
            ffi::Error::new(ffi::SQLITE_CONSTRAINT_UNIQUE),
            None,
        ));
        assert!(err.is_unique_violation());
        assert!(err.as_constraint_violation().unwrap().table().is_none());
    }

    #[test]
    fn other_constraint_failures_stay_driver_operation_failures() {
        for (code, message) in [
            (
                ffi::SQLITE_CONSTRAINT_NOTNULL,
                "NOT NULL constraint failed: users.email",
            ),
            (
                ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
                "FOREIGN KEY constraint failed",
            ),
            (ffi::SQLITE_CONSTRAINT_CHECK, "CHECK constraint failed: age"),
            (ffi::SQLITE_BUSY, "database is locked"),
        ] {
            let err = classify_sqlite_error(failure(code, message));
            assert!(err.is_driver_operation_failed(), "{message}");
            assert!(!err.is_constraint_violation(), "{message}");
        }
    }

    #[test]
    fn a_message_alone_does_not_make_a_unique_violation() {
        let err = classify_sqlite_error(failure(
            ffi::SQLITE_ERROR,
            "UNIQUE constraint failed: users.email",
        ));
        assert!(err.is_driver_operation_failed());
        assert!(!err.is_unique_violation());
    }
}
