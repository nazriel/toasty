//! The driver reports whether its connection is inside a transaction.

use std::sync::Arc;

use toasty_core::{
    Schema,
    driver::{Connection as _, Operation, operation::Transaction},
    schema::mapping::Mapping,
};
use toasty_driver_sqlite::Connection;

fn empty_schema() -> Arc<Schema> {
    Arc::new(Schema {
        app: Default::default(),
        db: Default::default(),
        mapping: Mapping {
            models: Default::default(),
            document_columns: Default::default(),
        },
    })
}

async fn run(connection: &mut Connection, schema: &Arc<Schema>, op: Transaction) -> bool {
    connection
        .exec(schema, Operation::Transaction(op))
        .await
        .is_ok()
}

/// `in_transaction` follows `BEGIN`, `COMMIT` and `ROLLBACK` through the
/// driver, savepoints keep it inside the enclosing transaction, and an
/// operation that fails leaves it as SQLite is.
#[tokio::test]
async fn in_transaction_follows_the_transaction_operations() {
    let schema = empty_schema();
    let mut connection = Connection::in_memory();
    assert_eq!(connection.in_transaction(), Some(false));

    assert!(run(&mut connection, &schema, Transaction::start()).await);
    assert_eq!(connection.in_transaction(), Some(true));

    assert!(
        run(
            &mut connection,
            &schema,
            Transaction::Savepoint("tx_1".into())
        )
        .await
    );
    assert!(
        run(
            &mut connection,
            &schema,
            Transaction::RollbackToSavepoint("tx_1".into())
        )
        .await
    );
    assert_eq!(connection.in_transaction(), Some(true));

    assert!(run(&mut connection, &schema, Transaction::Commit).await);
    assert_eq!(connection.in_transaction(), Some(false));

    assert!(run(&mut connection, &schema, Transaction::start()).await);
    assert!(run(&mut connection, &schema, Transaction::Rollback).await);
    assert_eq!(connection.in_transaction(), Some(false));

    // A rollback with no transaction fails, and there is still none.
    assert!(!run(&mut connection, &schema, Transaction::Rollback).await);
    assert_eq!(connection.in_transaction(), Some(false));

    // A failed operation inside a transaction leaves the transaction open.
    assert!(run(&mut connection, &schema, Transaction::start()).await);
    assert!(
        !run(
            &mut connection,
            &schema,
            Transaction::RollbackToSavepoint("missing".into())
        )
        .await
    );
    assert_eq!(connection.in_transaction(), Some(true));
}
