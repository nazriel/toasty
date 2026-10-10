use rusqlite::Connection as RusqliteConnection;
use std::{panic, sync::Arc};
use tokio::sync::Mutex;
use tracing::Dispatch;

/// A rusqlite connection whose calls run on tokio's blocking thread pool.
///
/// rusqlite calls block the calling thread, and SQLite's busy handler sleeps
/// it for up to the busy timeout while another connection holds a lock this
/// one needs. Run inline, that wait holds a runtime worker: with as many
/// waiting connections as workers (two on a current-thread runtime), the
/// connection holding the lock never gets a worker to reach its `COMMIT`, and
/// every waiter fails once its busy timeout expires. On the blocking pool, a
/// waiting connection parks a blocking thread instead, and the runtime's
/// workers stay free.
///
/// The blocking pool rather than a thread per connection: while a blocking
/// task runs, a current-thread runtime on a paused clock does not auto-advance
/// it, so time stands still during a statement, as it did when the statement
/// ran inline. A thread of the driver's own would look like an idle runtime
/// and let the clock jump to the next timer.
#[derive(Debug)]
pub(crate) struct BlockingConnection {
    connection: Arc<Mutex<RusqliteConnection>>,
}

impl BlockingConnection {
    pub(crate) fn new(connection: RusqliteConnection) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
        }
    }

    /// Runs `f` against the connection on the blocking thread pool and waits
    /// for the result without blocking the calling thread.
    ///
    /// Calls run one at a time, in the order they were made. `f` runs under
    /// the caller's tracing subscriber and inside its current span. A panic in
    /// `f` is resumed in the caller, as if `f` had run inline.
    ///
    /// Dropping the returned future once `f` has been handed to the blocking
    /// pool does not interrupt `f`: it runs to completion, its result is
    /// discarded, and only then is the connection free for the next call. The
    /// connection is never left in the middle of a statement.
    pub(crate) async fn call<T, F>(&self, f: F) -> toasty_core::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut RusqliteConnection) -> toasty_core::Result<T> + Send + 'static,
    {
        // Taken here rather than on the blocking thread: waiting for the
        // previous call holds no thread, and callers are served in order.
        let mut connection = self.connection.clone().lock_owned().await;

        // A blocking-pool thread has no subscriber or span of its own: carry
        // the caller's over, so the driver's events reach the same subscriber,
        // inside the caller's span tree, as they would if `f` ran inline.
        let dispatch = tracing::dispatcher::get_default(Dispatch::clone);
        let span = tracing::Span::current();

        let result = tokio::task::spawn_blocking(move || {
            tracing::dispatcher::with_default(&dispatch, || span.in_scope(|| f(&mut connection)))
        })
        .await;

        match result {
            Ok(result) => result,
            Err(err) => match err.try_into_panic() {
                Ok(payload) => panic::resume_unwind(payload),
                // The runtime is shutting down and dropped the call before it
                // started.
                Err(err) => Err(toasty_core::Error::connection_lost(err)),
            },
        }
    }
}
