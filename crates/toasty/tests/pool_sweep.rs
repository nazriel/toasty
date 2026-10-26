//! Deterministic tests for the pool's background sweep and recycle
//! behavior.
//!
//! Each test runs on paused tokio time against a fault-injecting
//! `Driver` that wraps a file-backed SQLite. The wrapper lets the
//! test arm a queue of "next N pings fail" / "next N execs fail"
//! tokens; everything else passes through to SQLite so the engine
//! can run real schema pushes and inserts. No real-time deadlines.

use std::{
    borrow::Cow,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU32, Ordering},
    },
    task::Poll,
    time::Duration,
};

use async_trait::async_trait;
use tempfile::TempDir;
use toasty_core::{
    Result, Schema,
    driver::{
        Capability, ConnectContext, Connection, Driver, ExecResponse, Operation,
        operation::Transaction,
    },
    schema::{
        db::{AppliedMigration, Migration},
        diff,
    },
};

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    #[auto]
    id: u64,
    name: String,
    age: i64,
}

#[derive(Debug, Default)]
struct MockState {
    pings: AtomicU32,
    ping_fail_tokens: AtomicU32,
    exec_fail_tokens: AtomicU32,
    /// Each token makes one `exec` panic once the inner driver is done.
    exec_panic_tokens: AtomicU32,
    /// Each token makes one `ROLLBACK` or `ROLLBACK TO SAVEPOINT` fail
    /// without reaching the inner driver, so its transaction stays open.
    rollback_fail_tokens: AtomicU32,
    /// Each token makes one `COMMIT` fail without reaching the inner driver.
    commit_fail_tokens: AtomicU32,
    /// How many `ROLLBACK`s were attempted, failed or not.
    rollbacks: AtomicU32,
    /// What `in_transaction` answers: one of the `REPORT_*` values.
    in_transaction_report: AtomicU8,
    connects: AtomicU32,
}

/// `in_transaction` answers what SQLite says.
const REPORT_INNER: u8 = 0;
/// `in_transaction` answers `Some(false)`, whatever SQLite says.
const REPORT_NOT_IN_TRANSACTION: u8 = 1;
/// `in_transaction` answers `None`, as a driver that cannot tell does.
const REPORT_UNKNOWN: u8 = 2;

#[derive(Debug)]
struct MockDriver {
    inner: toasty_driver_sqlite::Sqlite,
    state: Arc<MockState>,
    // Keeps the on-disk SQLite file alive for the lifetime of the
    // driver — the tempdir self-deletes on drop.
    _tempdir: TempDir,
}

impl MockDriver {
    fn new() -> Self {
        let tempdir = TempDir::new().expect("create tempdir");
        let path = tempdir.path().join("pool_sweep.db");
        Self {
            inner: toasty_driver_sqlite::Sqlite::open(&path),
            state: Arc::new(MockState::default()),
            _tempdir: tempdir,
        }
    }

    fn state(&self) -> Arc<MockState> {
        self.state.clone()
    }
}

#[async_trait]
impl Driver for MockDriver {
    fn url(&self) -> Cow<'_, str> {
        self.inner.url()
    }

    fn capability(&self) -> &'static Capability {
        self.inner.capability()
    }

    async fn connect(&self, cx: &ConnectContext) -> Result<Box<dyn Connection>> {
        let inner = self.inner.connect(cx).await?;
        self.state.connects.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(MockConnection {
            inner,
            state: self.state.clone(),
            valid: true,
        }))
    }

    fn generate_migration(&self, diff: &diff::Schema<'_>) -> Migration {
        self.inner.generate_migration(diff)
    }

    async fn reset_db(&self) -> Result<()> {
        self.inner.reset_db().await
    }
}

#[derive(Debug)]
struct MockConnection {
    inner: Box<dyn Connection>,
    state: Arc<MockState>,
    valid: bool,
}

#[async_trait]
impl Connection for MockConnection {
    async fn exec(&mut self, schema: &Arc<Schema>, op: Operation) -> Result<ExecResponse> {
        if self.state.exec_fail_tokens.load(Ordering::Relaxed) > 0 {
            self.state.exec_fail_tokens.fetch_sub(1, Ordering::Relaxed);
            self.valid = false;
            return Err(toasty_core::Error::connection_lost(std::io::Error::other(
                "mock exec failure",
            )));
        }
        if let Operation::Transaction(tx) = &op {
            if matches!(tx, Transaction::Rollback) {
                self.state.rollbacks.fetch_add(1, Ordering::Relaxed);
            }
            let tokens = match tx {
                Transaction::Rollback | Transaction::RollbackToSavepoint(_) => {
                    Some(&self.state.rollback_fail_tokens)
                }
                Transaction::Commit => Some(&self.state.commit_fail_tokens),
                _ => None,
            };
            if let Some(tokens) = tokens
                && tokens.load(Ordering::Relaxed) > 0
            {
                tokens.fetch_sub(1, Ordering::Relaxed);
                return Err(toasty_core::Error::driver_operation_failed(
                    std::io::Error::other("mock transaction failure"),
                ));
            }
        }
        let response = self.inner.exec(schema, op).await;
        if self.state.exec_panic_tokens.load(Ordering::Relaxed) > 0 {
            self.state.exec_panic_tokens.fetch_sub(1, Ordering::Relaxed);
            panic!("mock exec panic");
        }
        response
    }

    fn is_valid(&self) -> bool {
        self.valid && self.inner.is_valid()
    }

    fn in_transaction(&self) -> Option<bool> {
        match self.state.in_transaction_report.load(Ordering::Relaxed) {
            REPORT_NOT_IN_TRANSACTION => Some(false),
            REPORT_UNKNOWN => None,
            _ => self.inner.in_transaction(),
        }
    }

    async fn ping(&mut self) -> Result<()> {
        self.state.pings.fetch_add(1, Ordering::Relaxed);
        if self.state.ping_fail_tokens.load(Ordering::Relaxed) > 0 {
            self.state.ping_fail_tokens.fetch_sub(1, Ordering::Relaxed);
            self.valid = false;
            return Err(toasty_core::Error::connection_lost(std::io::Error::other(
                "mock ping failure",
            )));
        }
        self.inner.ping().await
    }

    async fn push_schema(&mut self, schema: &Schema) -> Result<()> {
        self.inner.push_schema(schema).await
    }

    async fn applied_migrations(&mut self) -> Result<Vec<AppliedMigration>> {
        self.inner.applied_migrations().await
    }

    async fn apply_migration(&mut self, id: u64, name: &str, migration: &Migration) -> Result<()> {
        self.inner.apply_migration(id, name, migration).await
    }
}

/// Build a `Db` with the given pool configuration. Returns the `Db`
/// and a handle to the mock's fault-injection / counter state.
async fn build_db(
    max_pool_size: usize,
    health_check_interval: Option<Duration>,
    pre_ping: bool,
) -> (toasty::Db, Arc<MockState>) {
    let driver = MockDriver::new();
    let state = driver.state();

    let db = toasty::Db::builder()
        .models(toasty::models!(User))
        .max_pool_size(max_pool_size)
        .pool_health_check_interval(health_check_interval)
        .pool_pre_ping(pre_ping)
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();
    (db, state)
}

/// Passive recovery: with the sweep disabled, a user-observed
/// `connection_lost` must still drain the dead slot so the next
/// call opens a fresh connection. Exercises the `Manager::recycle`
/// path in isolation.
#[tokio::test(start_paused = true)]
async fn pool_recovers_after_connection_lost() {
    let (mut db, state) = build_db(1, None, false).await;

    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    state.exec_fail_tokens.store(1, Ordering::Relaxed);

    let err = toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );

    toasty::create!(User {
        name: "carol",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
}

/// Periodic sweep evicts a silently-broken idle connection without
/// needing a user query to trip it.
#[tokio::test(start_paused = true)]
async fn sweep_evicts_dead_idle_connection() {
    let (mut db, state) = build_db(1, Some(Duration::from_millis(50)), false).await;

    // Force the pool to open its one connection.
    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(db.pool().status().size, 1);

    state.ping_fail_tokens.store(1, Ordering::Relaxed);

    // Advance past the first sweep tick.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert_eq!(
        db.pool().status().size,
        0,
        "sweep did not evict the dead idle connection",
    );

    // Next user query opens a fresh connection.
    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
}

/// One user query observing `connection_lost` must trigger an eager
/// sweep that pings every remaining idle connection. Otherwise each
/// queued fault would surface as a separate user-query failure.
#[tokio::test(start_paused = true)]
async fn eager_escalation_after_observed_loss() {
    // 60-second interval so the periodic tick cannot fire during the
    // test — only the notify-driven escalation path can drain the
    // queued faults here.
    let (mut db, state) = build_db(3, Some(Duration::from_secs(60)), false).await;

    let c1 = db.connection().await.unwrap();
    let c2 = db.connection().await.unwrap();
    let c3 = db.connection().await.unwrap();
    drop((c1, c2, c3));
    assert_eq!(db.pool().status().size, 3);

    // One fault for the user query, two for the sweep's escalation
    // pings over the remaining idle connections.
    state.exec_fail_tokens.store(1, Ordering::Relaxed);
    state.ping_fail_tokens.store(2, Ordering::Relaxed);

    let err = toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );

    // Yield long enough on the virtual clock for the sweep to
    // observe the wake() and run its escalate.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Sweep must have pinged both remaining idles. With a 60s
    // periodic interval, nothing else explains this — only the
    // notify-driven escalation path can have fired.
    assert_eq!(state.pings.load(Ordering::Relaxed), 2);
    // Both queued ping faults were consumed by the escalation pass.
    assert_eq!(state.ping_fail_tokens.load(Ordering::Relaxed), 0);

    // And the system recovers: the next queries succeed against
    // fresh connections (would surface connection_lost if the bad
    // idles were still around).
    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
    toasty::create!(User {
        name: "carol",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
}

/// A failing periodic ping self-wakes when it marks its connection bad,
/// which queues a sweep-notify permit before the same iteration
/// calls `escalate()`. The escalate snapshot includes that bump, so
/// on the next loop pass the queued permit must be deduped against
/// `last_serviced`. Without the dedup, every periodic-detected
/// failure produces a second escalate that re-pings every surviving
/// idle connection.
#[tokio::test(start_paused = true)]
async fn periodic_failure_does_not_redundantly_escalate() {
    let (db, state) = build_db(3, Some(Duration::from_secs(1)), false).await;

    let c1 = db.connection().await.unwrap();
    let c2 = db.connection().await.unwrap();
    let c3 = db.connection().await.unwrap();
    drop((c1, c2, c3));
    assert_eq!(db.pool().status().size, 3);

    state.ping_fail_tokens.store(1, Ordering::Relaxed);

    // Past the first tick (1s), well before the second (2s).
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    // 1 failing periodic ping + 2 healthy escalate pings = 3. Without
    // dedup, the queued notify permit would drive a second escalate
    // over the two remaining idles, raising the count to 5.
    assert_eq!(
        state.pings.load(Ordering::Relaxed),
        3,
        "expected one escalation pass (1 fail + 2 healthy); redundant escalate would have raised this to 5",
    );
}

/// Pre-ping catches a silently-broken idle connection on checkout
/// and the caller never sees the failure. Sweep is disabled so the
/// only path that can drain the dead slot before the user's query
/// goes out is `Manager::recycle`'s active ping.
#[tokio::test(start_paused = true)]
async fn pre_ping_evicts_silently_broken_idle_connection() {
    let (mut db, state) = build_db(1, None, true).await;

    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(db.pool().status().size, 1);

    let pings_before = state.pings.load(Ordering::Relaxed);
    state.ping_fail_tokens.store(1, Ordering::Relaxed);

    // Recycle pings, the ping fails, deadpool drops the slot and
    // opens a fresh one before the user's exec runs.
    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    assert_eq!(state.pings.load(Ordering::Relaxed) - pings_before, 1);
    assert_eq!(state.ping_fail_tokens.load(Ordering::Relaxed), 0);
    assert_eq!(db.pool().status().size, 1);
}

/// Pre-ping issues a round-trip on every checkout. The first query
/// after build may or may not recycle (depending on whether the
/// build-time probe left a slot warm), so we measure pings against a
/// snapshot taken once the pool is steady-state.
#[tokio::test(start_paused = true)]
async fn pre_ping_runs_on_every_checkout() {
    let (mut db, state) = build_db(1, None, true).await;

    // Warm the pool past any setup-driven acquires.
    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    let pings_before = state.pings.load(Ordering::Relaxed);

    // Two more checkouts — each must recycle the lone idle slot and
    // therefore fire exactly one pre-ping.
    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
    toasty::create!(User {
        name: "carol",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    assert_eq!(state.pings.load(Ordering::Relaxed) - pings_before, 2);
}

/// Lifetime cap: a connection older than `pool_max_connection_lifetime`
/// is evicted by `recycle` and replaced with a fresh one. Uses real
/// time because deadpool's `Metrics::age` reads `std::time::Instant`,
/// which `tokio::time::pause` does not advance.
#[tokio::test]
async fn lifetime_cap_evicts_aged_connection() {
    let driver = MockDriver::new();
    let state = driver.state();

    let mut db = toasty::Db::builder()
        .models(toasty::models!(User))
        .max_pool_size(1)
        .pool_health_check_interval(None)
        .pool_max_connection_lifetime(Some(Duration::from_millis(50)))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    let connects_before = state.connects.load(Ordering::Relaxed);

    // Real-time wait past the lifetime cap.
    tokio::time::sleep(Duration::from_millis(150)).await;

    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    assert_eq!(
        state.connects.load(Ordering::Relaxed) - connects_before,
        1,
        "recycle did not replace the over-lifetime connection",
    );
    assert_eq!(db.pool().status().size, 1);
}

/// Idle-time cap: a connection that has been sitting unused for longer
/// than `pool_max_connection_idle_time` is evicted by `recycle` and
/// replaced on the next checkout.
#[tokio::test]
async fn idle_time_cap_evicts_long_idle_connection() {
    let driver = MockDriver::new();
    let state = driver.state();

    let mut db = toasty::Db::builder()
        .models(toasty::models!(User))
        .max_pool_size(1)
        .pool_health_check_interval(None)
        .pool_max_connection_idle_time(Some(Duration::from_millis(50)))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    let connects_before = state.connects.load(Ordering::Relaxed);

    // Real-time wait past the idle cap.
    tokio::time::sleep(Duration::from_millis(150)).await;

    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    assert_eq!(
        state.connects.load(Ordering::Relaxed) - connects_before,
        1,
        "recycle did not replace the over-idle connection",
    );
    assert_eq!(db.pool().status().size, 1);
}

/// A connection within both caps is reused — `recycle` must not evict
/// a healthy young connection.
#[tokio::test]
async fn lifetime_and_idle_caps_do_not_evict_fresh_connection() {
    let driver = MockDriver::new();
    let state = driver.state();

    let mut db = toasty::Db::builder()
        .models(toasty::models!(User))
        .max_pool_size(1)
        .pool_health_check_interval(None)
        .pool_max_connection_lifetime(Some(Duration::from_secs(60)))
        .pool_max_connection_idle_time(Some(Duration::from_secs(60)))
        .build(driver)
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    let connects_before = state.connects.load(Ordering::Relaxed);

    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();

    assert_eq!(
        state.connects.load(Ordering::Relaxed),
        connects_before,
        "recycle replaced a connection within both caps",
    );
}

/// Polls `future` once, so that it starts, and returns whether it finished.
async fn poll_once<F: Future + Unpin>(future: &mut F) -> bool {
    std::future::poll_fn(|cx| Poll::Ready(Pin::new(&mut *future).poll(cx).is_ready())).await
}

/// A statement whose caller stops waiting finishes on a spawned task. If
/// that rest panics inside a transaction, the connection is marked bad:
/// the statement queued behind it fails with `connection_lost` instead of
/// running inside the abandoned transaction (and being lost with it when
/// the connection closes), and nothing of the transaction is committed.
#[tokio::test(start_paused = true)]
async fn statement_queued_behind_a_panicked_operation_is_connection_lost() {
    let (mut db, state) = build_db(1, None, false).await;

    let mut conn = db.connection().await.unwrap();
    let mut tx = conn.transaction().await.unwrap();
    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut tx)
    .await
    .unwrap();

    let mut bob = Box::pin(
        toasty::create!(User {
            name: "bob",
            age: 30
        })
        .exec(&mut tx),
    );
    assert!(!poll_once(&mut bob).await, "the insert is in flight");
    // The rest of the insert, now on a spawned task, panics once the
    // driver is done with it.
    state.exec_panic_tokens.store(1, Ordering::Relaxed);
    drop(bob);

    let err = toasty::create!(User {
        name: "carol",
        age: 30
    })
    .exec(&mut tx)
    .await
    .unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );
    assert_eq!(state.exec_panic_tokens.load(Ordering::Relaxed), 0);

    drop(tx);
    drop(conn);

    // The bad connection left the pool; a fresh one sees nothing of the
    // abandoned transaction.
    let connects_before = state.connects.load(Ordering::Relaxed);
    assert!(User::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(state.connects.load(Ordering::Relaxed), connects_before + 1);
}

/// Writes a row in a transaction that is dropped while its rollback fails
/// (the driver's next `ROLLBACK` never reaches SQLite, so the transaction
/// stays open), with the driver answering `in_transaction` as `report` says.
/// Then reads on the same connection.
///
/// Returns the read's result, the pool and the number of connections opened
/// before the read.
async fn read_after_failed_rollback(
    report: u8,
) -> (toasty::Db, Arc<MockState>, toasty::Result<usize>, u32) {
    let (db, state) = build_db(1, None, false).await;
    state.in_transaction_report.store(report, Ordering::Relaxed);

    let mut conn = db.connection().await.unwrap();
    let mut tx = conn.transaction().await.unwrap();
    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut tx)
    .await
    .unwrap();

    state.rollback_fail_tokens.store(1, Ordering::Relaxed);
    drop(tx);

    let read = User::all().exec(&mut conn).await.map(|users| users.len());
    assert_eq!(state.rollback_fail_tokens.load(Ordering::Relaxed), 0);
    drop(conn);

    let connects = state.connects.load(Ordering::Relaxed);
    (db, state, read, connects)
}

/// A dropped transaction whose rollback fails, with the driver still inside
/// it, leaves the connection bad: the next operation on it fails with
/// `connection_lost` instead of reading the transaction's uncommitted row,
/// and the pool replaces the connection, which sees nothing of it.
#[tokio::test(start_paused = true)]
async fn failed_deferred_rollback_in_a_transaction_discards_the_connection() {
    let (mut db, state, read, connects) = read_after_failed_rollback(REPORT_INNER).await;

    let err = read.unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );

    assert!(User::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(state.connects.load(Ordering::Relaxed), connects + 1);
}

/// A rollback that fails with the driver outside any transaction (the stray
/// rollback after a cancelled `COMMIT`) is harmless: the connection stays in
/// the pool.
#[tokio::test(start_paused = true)]
async fn failed_deferred_rollback_outside_a_transaction_keeps_the_connection() {
    let (mut db, state, read, connects) =
        read_after_failed_rollback(REPORT_NOT_IN_TRANSACTION).await;

    // The mock reports no transaction while SQLite is still inside one, so
    // the read sees the row.
    assert_eq!(read.unwrap(), 1);

    User::all().exec(&mut db).await.unwrap();
    assert_eq!(state.connects.load(Ordering::Relaxed), connects);
}

/// A driver that cannot tell whether it is inside a transaction is assumed to
/// be: a failed deferred rollback discards the connection.
#[tokio::test(start_paused = true)]
async fn failed_deferred_rollback_on_a_driver_that_cannot_tell_discards_the_connection() {
    let (mut db, state, read, connects) = read_after_failed_rollback(REPORT_UNKNOWN).await;

    let err = read.unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );

    User::all().exec(&mut db).await.unwrap();
    assert_eq!(state.connects.load(Ordering::Relaxed), connects + 1);
}

/// A plan the engine wraps in its own transaction, whose `COMMIT` fails:
/// the engine rolls back, returns the commit's error, and the connection,
/// outside any transaction again, stays in the pool without the batch's rows.
#[tokio::test(start_paused = true)]
async fn failed_plan_commit_is_rolled_back() {
    let (mut db, state) = build_db(1, None, false).await;
    let mut conn = db.connection().await.unwrap();
    let connects = state.connects.load(Ordering::Relaxed);
    let rollbacks = state.rollbacks.load(Ordering::Relaxed);

    state.commit_fail_tokens.store(1, Ordering::Relaxed);
    let err = toasty::batch((
        toasty::create!(User {
            name: "alice",
            age: 30
        }),
        toasty::create!(User {
            name: "bob",
            age: 30
        }),
    ))
    .exec(&mut conn)
    .await
    .unwrap_err();
    assert!(!err.is_connection_lost(), "got {err}");
    assert_eq!(state.commit_fail_tokens.load(Ordering::Relaxed), 0);
    assert_eq!(state.rollbacks.load(Ordering::Relaxed), rollbacks + 1);
    drop(conn);

    assert!(User::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(state.connects.load(Ordering::Relaxed), connects);
}

/// The same, with the rollback failing too: the connection is still inside
/// the plan's transaction, so it leaves the pool.
#[tokio::test(start_paused = true)]
async fn failed_plan_commit_and_rollback_discard_the_connection() {
    let (mut db, state) = build_db(1, None, false).await;
    let mut conn = db.connection().await.unwrap();
    let connects = state.connects.load(Ordering::Relaxed);

    state.commit_fail_tokens.store(1, Ordering::Relaxed);
    state.rollback_fail_tokens.store(1, Ordering::Relaxed);
    let err = toasty::batch((
        toasty::create!(User {
            name: "alice",
            age: 30
        }),
        toasty::create!(User {
            name: "bob",
            age: 30
        }),
    ))
    .exec(&mut conn)
    .await
    .unwrap_err();
    assert!(!err.is_connection_lost(), "got {err}");
    assert_eq!(state.rollback_fail_tokens.load(Ordering::Relaxed), 0);

    let err = User::all().exec(&mut conn).await.unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );
    drop(conn);

    assert!(User::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(state.connects.load(Ordering::Relaxed), connects + 1);
}

/// A nested commit cancelled in flight still releases its savepoint, and the
/// dropped handle then queues a rollback to a savepoint that is gone. That
/// rollback fails, but the outer transaction is open and owned: the
/// connection stays usable, and the outer transaction writes, commits and
/// leaves its rows behind.
#[tokio::test(start_paused = true)]
async fn cancelled_nested_commit_leaves_the_outer_transaction_usable() {
    let (mut db, state) = build_db(1, None, false).await;
    let connects = state.connects.load(Ordering::Relaxed);

    let mut conn = db.connection().await.unwrap();
    let mut tx = conn.transaction().await.unwrap();
    toasty::create!(User {
        name: "alice",
        age: 30
    })
    .exec(&mut tx)
    .await
    .unwrap();

    {
        let nested = tx.transaction().await.unwrap();
        let mut commit = Box::pin(nested.commit());
        assert!(!poll_once(&mut commit).await, "the commit is in flight");
        drop(commit);
    }

    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    drop(conn);

    assert_eq!(User::all().exec(&mut db).await.unwrap().len(), 2);
    assert_eq!(state.connects.load(Ordering::Relaxed), connects);
}

/// Transaction control sent as raw SQL leaves a transaction no `Transaction`
/// owns: the connection is discarded, and the next operation on it says why.
#[tokio::test(start_paused = true)]
async fn raw_begin_leaves_a_transaction_nobody_owns() {
    let (mut db, state) = build_db(1, None, false).await;

    let mut conn = db.connection().await.unwrap();
    toasty::sql::statement("BEGIN")
        .exec(&mut conn)
        .await
        .unwrap();

    let err = User::all().exec(&mut conn).await.unwrap_err();
    assert!(
        err.is_connection_lost(),
        "expected connection_lost, got {err}"
    );
    assert!(err.to_string().contains("no Transaction owns"), "got {err}");
    drop(conn);

    let connects = state.connects.load(Ordering::Relaxed);
    assert!(User::all().exec(&mut db).await.unwrap().is_empty());
    assert_eq!(state.connects.load(Ordering::Relaxed), connects + 1);
}

/// A statement that panics while its caller polls it leaves the connection
/// marked bad, and the pool replaces it.
#[tokio::test(start_paused = true)]
async fn connection_is_replaced_after_a_statement_panics_in_its_caller() {
    let (db, state) = build_db(1, None, false).await;

    state.exec_panic_tokens.store(1, Ordering::Relaxed);
    let result = tokio::spawn({
        let mut db = db.clone();
        async move {
            toasty::create!(User {
                name: "alice",
                age: 30
            })
            .exec(&mut db)
            .await
        }
    })
    .await;
    assert!(result.unwrap_err().is_panic());

    let connects_before = state.connects.load(Ordering::Relaxed);
    let mut db = db;
    toasty::create!(User {
        name: "bob",
        age: 30
    })
    .exec(&mut db)
    .await
    .unwrap();
    assert_eq!(state.connects.load(Ordering::Relaxed), connects_before + 1);
}
