#![cfg(feature = "sqlite")]

struct SqliteSetup;

impl SqliteSetup {
    fn new() -> Self {
        SqliteSetup
    }
}

#[async_trait::async_trait]
impl toasty_driver_integration_suite::Setup for SqliteSetup {
    fn driver(&self) -> Box<dyn toasty_core::driver::Driver> {
        Box::new(toasty_driver_sqlite::Sqlite::in_memory())
    }

    async fn delete_table(&self, _name: &str) {
        // There is no need to delete anything since the driver operates in-memory
    }
}

// Generate all driver tests
toasty_driver_integration_suite::generate_driver_tests!(
    SqliteSetup::new(),
    native_decimal: false,
    bigdecimal_implemented: false,
    decimal_arbitrary_precision: false,
    native_timestamp: false,
    native_date: false,
    native_time: false,
    native_datetime: false,
    native_cidr: false,
    native_inet: false,
    native_macaddr: false,
    native_macaddr8: false,
    native_ilike: false,
    native_json: false,
    native_jsonb: false,
    native_array: false,
    native_enum: false,
    vec_scalar: true,
    unique_list_index: false,
    document_collections: true,
    vec_remove: false,
    vec_pop: false,
    vec_remove_at: false,
    test_connection_pool: false,
);

#[derive(Debug, toasty::Model)]
struct PoolItem {
    #[key]
    #[auto]
    id: i64,
}

/// In-memory SQLite forces `max_connections = 1`; verify the driver cap
/// overrides a larger user-requested pool size.
#[tokio::test]
async fn in_memory_caps_user_max_pool_size() {
    let db = toasty::Db::builder()
        .models(toasty::models!(PoolItem))
        .max_pool_size(16)
        .build(toasty_driver_sqlite::Sqlite::in_memory())
        .await
        .unwrap();

    assert_eq!(db.pool().status().max_size, 1);
}

#[tokio::test]
async fn auto_i64_key_uses_sqlite_integer_storage_type() {
    let db = toasty::Db::builder()
        .models(toasty::models!(PoolItem))
        .build(toasty_driver_sqlite::Sqlite::in_memory())
        .await
        .unwrap();

    let id = &db.schema().db.tables[0].columns[0];

    assert_eq!(id.storage_ty, toasty::schema::db::Type::Integer(4));
    assert!(id.auto_increment);
}

// Concurrent writers on a file database. SQLite's busy handler sleeps the
// calling thread while another connection holds the write lock, so these
// tests check that a writer waiting for the lock never holds a runtime
// worker: the transaction holding the lock must still be able to reach its
// `COMMIT`, and unrelated tasks must keep running.

mod contention {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use toasty_core::driver::operation::TransactionMode;

    #[derive(Debug, toasty::Model)]
    struct Entry {
        #[key]
        #[auto]
        id: i64,
        writer: i64,
    }

    /// rusqlite's default busy timeout is 5 s. A writer that waits for the
    /// lock and fails would only do so after that long, so every test here
    /// expects its writers to be done well before it.
    const WELL_INSIDE_BUSY_TIMEOUT: Duration = Duration::from_secs(3);

    /// How long each writer keeps the write lock, yielding to the runtime,
    /// so the other writers queue up behind it.
    const HOLD: Duration = Duration::from_millis(20);

    async fn file_db(dir: &tempfile::TempDir, pool_size: usize) -> toasty::Db {
        let db = toasty::Db::builder()
            .models(toasty::models!(Entry))
            .max_pool_size(pool_size)
            .build(toasty_driver_sqlite::Sqlite::open(
                dir.path().join("db.sqlite3"),
            ))
            .await
            .unwrap();
        db.push_schema().await.unwrap();
        db
    }

    async fn write_in_immediate_tx(db: &mut toasty::Db, writer: i64) -> toasty::Result<()> {
        let mut tx = db
            .transaction_builder()
            .mode(TransactionMode::Immediate)
            .begin()
            .await?;
        Entry::create().writer(writer).exec(&mut tx).await?;
        tokio::time::sleep(HOLD).await;
        tx.commit().await
    }

    async fn count_entries(db: &mut toasty::Db) -> usize {
        Entry::all().exec(db).await.unwrap().len()
    }

    #[test]
    fn immediate_transactions_outnumbering_runtime_workers_all_commit() {
        const WORKERS: usize = 2;
        const WRITERS: usize = 8;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKERS)
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let mut db = file_db(&dir, WRITERS).await;

            let writers: Vec<_> = (0..WRITERS as i64)
                .map(|writer| {
                    let mut db = db.clone();
                    tokio::spawn(async move { write_in_immediate_tx(&mut db, writer).await })
                })
                .collect();

            let results = tokio::time::timeout(WELL_INSIDE_BUSY_TIMEOUT, async {
                let mut results = vec![];
                for writer in writers {
                    results.push(writer.await.unwrap());
                }
                results
            })
            .await
            .expect("writers waiting for the lock starved the runtime");

            for result in results {
                result.expect("every immediate transaction commits");
            }
            assert_eq!(count_entries(&mut db).await, WRITERS);
        });
    }

    #[tokio::test]
    async fn racing_immediate_transactions_commit_on_a_current_thread_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = file_db(&dir, 2).await;
        let mut db_a = db.clone();
        let mut db_b = db.clone();

        let (a, b) = tokio::time::timeout(WELL_INSIDE_BUSY_TIMEOUT, async {
            tokio::join!(
                write_in_immediate_tx(&mut db_a, 0),
                write_in_immediate_tx(&mut db_b, 1),
            )
        })
        .await
        .expect("the waiting writer blocked the runtime");

        a.expect("the first immediate transaction commits");
        b.expect("the second immediate transaction commits");
        assert_eq!(count_entries(&mut db).await, 2);
    }

    #[tokio::test]
    async fn runtime_keeps_running_while_a_writer_waits_for_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = file_db(&dir, 2).await;

        let mut holder = db.clone();
        let mut tx = holder
            .transaction_builder()
            .mode(TransactionMode::Immediate)
            .begin()
            .await
            .unwrap();
        Entry::create().writer(0).exec(&mut tx).await.unwrap();

        let mut waiter_db = db.clone();
        let waiter = tokio::spawn(async move { write_in_immediate_tx(&mut waiter_db, 1).await });

        let ticks = Arc::new(AtomicUsize::new(0));
        let ticker = tokio::spawn({
            let ticks = ticks.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    ticks.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(250)).await;
        let ticked = ticks.load(Ordering::Relaxed);
        assert!(
            ticked >= 10,
            "the runtime stalled while a writer waited for the lock: {ticked} ticks in 250 ms"
        );
        assert!(
            !waiter.is_finished(),
            "the second writer should still be waiting for the lock"
        );

        tx.commit().await.unwrap();
        tokio::time::timeout(WELL_INSIDE_BUSY_TIMEOUT, waiter)
            .await
            .expect("the waiting writer gets the lock once it is released")
            .unwrap()
            .expect("the waiting writer commits");
        ticker.abort();

        assert_eq!(count_entries(&mut db).await, 2);
    }
}
