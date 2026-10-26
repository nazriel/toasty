//! A pooled connection, driven by the task that uses it.
//!
//! Each pooled connection owns its driver connection behind an async mutex.
//! A caller locks it and runs the engine and the driver on its own task, so a
//! statement crosses no thread to reach the connection. The mutex serializes
//! access to the driver connection — drivers are not required to be `Sync` on
//! `exec` — and serves callers in the order they asked.
//!
//! An operation, once started, runs to completion: if the caller drops its
//! future part-way, the rest of the operation moves to a spawned task. A
//! connection is never left in the middle of a statement, nor of a
//! transaction the engine opened for one. A [`Transaction`](super::Transaction)
//! dropped without commit or rollback queues its rollback with
//! [`PooledConnection::defer`]: the connection's next operation runs it before
//! its own, so nothing reaches the connection ahead of it.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, ready},
};

use toasty_core::driver::operation::{Operation, RawSql};
use toasty_core::driver::{Connection, ExecResponse, Rows};
#[cfg(feature = "migration")]
use toasty_core::schema::db::{AppliedMigration, Migration};
use toasty_core::stmt::Value;
use tokio::sync::{Mutex, MutexGuard};
use tracing::{Instrument, instrument::WithSubscriber};

use super::pool::SweepWaker;
use crate::engine::Engine;

/// A driver connection held by the pool, with the state the pool needs to
/// decide whether it can be reused.
pub(crate) struct PooledConnection {
    slot: Arc<Slot>,
}

struct Slot {
    held: Mutex<Held>,
    engine: Engine,

    /// Operations queued by [`PooledConnection::defer`], oldest first, with
    /// the span each was queued in.
    deferred: std::sync::Mutex<VecDeque<(Operation, tracing::Span)>>,

    /// How many operations [`PooledConnection::defer`] has queued so far.
    /// Incremented after the push, while `deferred` is still locked.
    deferred_queued: AtomicU64,

    /// Set once the connection went bad; the pool's `recycle` evicts it.
    invalid: AtomicBool,

    /// Woken when the connection goes bad, so the pool's health-check sweep
    /// can escalate immediately rather than waiting for its next periodic
    /// tick.
    sweep_waker: Arc<SweepWaker>,
}

/// What the slot's mutex guards.
struct Held {
    connection: Box<dyn Connection>,

    /// How many deferred operations have run on the connection.
    deferred_run: u64,
}

impl PooledConnection {
    pub(crate) fn new(
        connection: Box<dyn Connection>,
        engine: Engine,
        sweep_waker: Arc<SweepWaker>,
    ) -> Self {
        Self {
            slot: Arc::new(Slot {
                held: Mutex::new(Held {
                    connection,
                    deferred_run: 0,
                }),
                engine,
                deferred: std::sync::Mutex::new(VecDeque::new()),
                deferred_queued: AtomicU64::new(0),
                invalid: AtomicBool::new(false),
                sweep_waker,
            }),
        }
    }

    /// Returns true once the connection went bad: the driver reported it
    /// invalid after an operation, or an operation panicked. Used by the
    /// pool's `recycle` to detect dead slots.
    pub(crate) fn is_invalid(&self) -> bool {
        self.slot.invalid.load(Ordering::Acquire)
    }

    /// Execute a statement (compile + run on the connection) inside `span`.
    pub(crate) async fn exec_statement(
        &self,
        stmt: toasty_core::stmt::Statement,
        in_transaction: bool,
        span: tracing::Span,
    ) -> crate::Result<ExecResponse> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                exec_statement(&slot.engine, &mut **connection, stmt, in_transaction).await
            }
            .instrument(span),
        )
        .await
    }

    pub(crate) async fn exec_operation(&self, operation: Operation) -> crate::Result<ExecResponse> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                connection.exec(&slot.engine.schema, operation).await
            }
            .in_current_span(),
        )
        .await
    }

    /// Execute user-authored SQL through the engine's SQL capability guard,
    /// inside `span`.
    pub(crate) async fn exec_raw_sql(
        &self,
        raw: RawSql,
        span: tracing::Span,
    ) -> crate::Result<ExecResponse> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                slot.engine.exec_raw_sql(&mut **connection, raw).await
            }
            .instrument(span),
        )
        .await
    }

    pub(crate) async fn push_schema(&self) -> crate::Result<()> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                connection.push_schema(&slot.engine.schema).await
            }
            .in_current_span(),
        )
        .await
    }

    #[cfg(feature = "migration")]
    pub(crate) async fn applied_migrations(&self) -> crate::Result<Vec<AppliedMigration>> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                connection.applied_migrations().await
            }
            .in_current_span(),
        )
        .await
    }

    #[cfg(feature = "migration")]
    pub(crate) async fn apply_migration(
        &self,
        id: u64,
        name: String,
        migration: Migration,
    ) -> crate::Result<()> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                connection.apply_migration(id, &name, &migration).await
            }
            .in_current_span(),
        )
        .await
    }

    /// Active liveness probe issued by the pool's background sweep and by
    /// the per-acquire pre-ping. Takes the connection's lock like any other
    /// operation, so the ping is serialized against an in-flight `exec` on
    /// the same connection.
    pub(crate) async fn ping(&self) -> crate::Result<()> {
        let slot = self.slot.clone();
        self.run(
            async move {
                let mut connection = slot.lock().await?;
                connection.ping().await
            }
            .in_current_span(),
        )
        .await
    }

    /// Queue `operation` without waiting for it, for a caller that cannot
    /// await (a `Drop`). It runs after every operation started before this
    /// call and before any operation started after it; its result is
    /// discarded.
    ///
    /// A spawned task also takes the connection's lock right away, so the
    /// operation runs now even if no one uses the connection again soon:
    /// a dropped transaction should not keep its locks until the
    /// connection's next use.
    pub(crate) fn defer(&self, operation: Operation) {
        {
            let mut deferred = self.slot.deferred.lock().unwrap();
            deferred.push_back((operation, tracing::Span::current()));
            self.slot.deferred_queued.fetch_add(1, Ordering::Release);
        }

        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let slot = self.slot.clone();
            let drain = Box::pin(async move {
                // A connection that went bad runs nothing more; it is
                // evicted, and closing it ends its transaction.
                let _ = slot.lock().await;
            });
            runtime.spawn(
                InvalidateUnlessDone::new(drain, self.slot.clone()).with_current_subscriber(),
            );
        }
    }

    /// Runs `operation` on the caller's task, to completion even if the
    /// caller stops waiting for it.
    fn run<F>(&self, operation: F) -> RunToCompletion<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        RunToCompletion {
            operation: Some(Box::pin(operation)),
            polling: false,
            slot: self.slot.clone(),
        }
    }
}

impl std::fmt::Debug for PooledConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledConnection")
            .field("invalid", &self.is_invalid())
            .finish()
    }
}

impl Slot {
    /// Waits for the connection, then runs the operations
    /// [`PooledConnection::defer`] queued before this call and that have not
    /// run yet. Operations queued while this call waits are left for a later
    /// lock, as they come after the operation this call is for.
    ///
    /// Fails with `connection_lost` once the connection went bad, whether
    /// before this call or while it ran the deferred operations: nothing more
    /// runs on it.
    async fn lock(&self) -> crate::Result<Locked<'_>> {
        let queued = self.deferred_queued.load(Ordering::Acquire);
        let held = self.held.lock().await;

        // An operation that held the connection before may have panicked, or
        // been dropped where it stood, anywhere: even inside a transaction it
        // opened. A statement run now could land in that transaction and be
        // lost with it when the connection closes.
        if self.invalid.load(Ordering::Acquire) {
            return Err(connection_went_bad());
        }

        let mut locked = Locked { slot: self, held };

        while locked.held.deferred_run < queued {
            let (operation, span) = self
                .deferred
                .lock()
                .unwrap()
                .pop_front()
                .expect("a counted deferred operation is queued");
            locked.held.deferred_run += 1;
            let result = locked
                .held
                .connection
                .exec(&self.engine.schema, operation)
                .instrument(span)
                .await;
            if let Err(err) = result {
                tracing::debug!(error = %err, "deferred operation failed");
            }
            if !locked.held.connection.is_valid() {
                // `locked` marks the slot as it drops.
                return Err(connection_went_bad());
            }
        }

        Ok(locked)
    }

    /// Marks the connection bad. The first time, also wakes the pool's
    /// sweep: a real connection-lost error usually means more than one
    /// connection in the pool is affected (database restart, network
    /// event), and catching the rest eagerly turns a multi-second recovery
    /// window into one round-trip per idle slot.
    fn invalidate(&self) {
        if !self.invalid.swap(true, Ordering::AcqRel) {
            self.sweep_waker.wake();
        }
    }
}

/// The locked driver connection.
///
/// Dropping it checks whether the connection is still valid, before the
/// operation's result reaches the caller: a caller woken by the result that
/// then returns the connection to the pool sees it marked in
/// `Manager::recycle`, and the slot is evicted.
struct Locked<'a> {
    slot: &'a Slot,
    held: MutexGuard<'a, Held>,
}

impl std::ops::Deref for Locked<'_> {
    type Target = Box<dyn Connection>;

    fn deref(&self) -> &Self::Target {
        &self.held.connection
    }
}

impl std::ops::DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.held.connection
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        // A panic may have left the connection anywhere, even mid-statement.
        if std::thread::panicking() {
            tracing::debug!("operation panicked; discarding connection");
            self.slot.invalidate();
        } else if !self.held.connection.is_valid() {
            tracing::debug!("connection reported invalid; discarding it");
            self.slot.invalidate();
        }
    }
}

/// The error for an operation on a connection that went bad before it.
fn connection_went_bad() -> crate::Error {
    toasty_core::Error::connection_lost(std::io::Error::other(
        "the connection went bad while an earlier operation used it",
    ))
}

/// A connection operation that runs on the caller's task while the caller
/// polls it. Dropped before it finishes, it hands the rest of the operation
/// to a spawned task, under the dropping caller's subscriber, and discards
/// its output.
struct RunToCompletion<F: Future + Send + 'static>
where
    F::Output: Send + 'static,
{
    operation: Option<Pin<Box<F>>>,

    /// Set while `operation` is being polled. Still set in `drop` only if
    /// that poll panicked: the operation is then poisoned and cannot be
    /// resumed.
    polling: bool,

    slot: Arc<Slot>,
}

impl<F> Future for RunToCompletion<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = &mut *self;
        let operation = this
            .operation
            .as_mut()
            .expect("operation polled after completion");
        this.polling = true;
        let poll = operation.as_mut().poll(cx);
        this.polling = false;
        let output = ready!(poll);
        this.operation = None;
        Poll::Ready(output)
    }
}

impl<F> Drop for RunToCompletion<F>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn drop(&mut self) {
        let Some(operation) = self.operation.take() else {
            return;
        };
        if self.polling {
            // The operation panicked mid-poll, wherever it stood.
            self.slot.invalidate();
            return;
        }
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(
                    InvalidateUnlessDone::new(operation, self.slot.clone())
                        .with_current_subscriber(),
                );
            }
            Err(_) => {
                // No runtime to finish on: the operation is dropped wherever
                // it stood, so the connection can no longer be trusted.
                self.slot.invalidate();
            }
        }
    }
}

/// A spawned connection task (the rest of a dropped operation, or a
/// deferred-operation drain). If it is dropped before it finishes — its
/// runtime shut down, or it panicked — the connection may be anywhere, so
/// the slot is marked bad.
struct InvalidateUnlessDone<F: Future> {
    future: Pin<Box<F>>,
    done: bool,
    slot: Arc<Slot>,
}

impl<F: Future> InvalidateUnlessDone<F> {
    fn new(future: Pin<Box<F>>, slot: Arc<Slot>) -> Self {
        Self {
            future,
            done: false,
            slot,
        }
    }
}

impl<F: Future> Future for InvalidateUnlessDone<F> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        ready!(self.future.as_mut().poll(cx));
        self.done = true;
        Poll::Ready(())
    }
}

impl<F: Future> Drop for InvalidateUnlessDone<F> {
    fn drop(&mut self) {
        if !self.done {
            self.slot.invalidate();
        }
    }
}

async fn exec_statement(
    engine: &Engine,
    connection: &mut dyn Connection,
    stmt: toasty_core::stmt::Statement,
    in_transaction: bool,
) -> crate::Result<ExecResponse> {
    let single = stmt.is_single();
    let mut response = engine.exec(connection, stmt, in_transaction).await?;
    response.values.buffer().await?;

    if single {
        let Rows::Value(Value::List(mut items)) = response.values else {
            unreachable!()
        };
        assert!(
            items.len() <= 1,
            "expected at most 1 row for single statement, got {}",
            items.len()
        );
        response.values = Rows::Value(items.pop().unwrap_or(Value::Null));
    }

    Ok(response)
}
