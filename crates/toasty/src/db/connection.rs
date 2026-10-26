use super::Transaction;
use super::pool::Manager;
use super::pooled::PooledConnection;
use super::tx::ConnRef;

use async_trait::async_trait;
use std::sync::Arc;
#[cfg(feature = "migration")]
use toasty_core::schema::db::{AppliedMigration, Migration};
use toasty_core::{
    Schema,
    driver::{
        Capability, ExecResponse,
        operation::{Operation, RawSql},
    },
    stmt,
};

/// A dedicated database connection retrieved from a pool.
///
/// Holding a `Connection` guarantees that all operations are executed on the
/// same physical connection. This is useful when multiple statements must
/// share connection-level state such as temporary tables or session variables.
///
/// When dropped, the connection is returned to the pool for reuse.
pub struct Connection {
    /// Always `Some` until `drop`.
    pub(super) inner: Option<deadpool::managed::Object<Manager>>,
    pub(super) shared: Arc<super::Shared>,
}

impl Connection {
    /// Access the underlying pooled connection.
    pub(crate) fn pooled(&self) -> &PooledConnection {
        self.inner.as_ref().expect("connection used after drop")
    }

    /// Returns the compiled schema used by this connection.
    pub fn schema(&self) -> &Arc<Schema> {
        &self.shared.engine.schema
    }

    pub(crate) async fn exec_stmt(
        &self,
        stmt: stmt::Statement,
        in_transaction: bool,
    ) -> crate::Result<ExecResponse> {
        // Created before the statement moves into the pooled connection's
        // operation, so the span parents to the caller's current span.
        let span = crate::instrument::query_span(&self.shared.engine.schema, &stmt);
        self.pooled()
            .exec_statement(stmt, in_transaction, span)
            .await
    }

    pub(crate) async fn exec_operation(&self, operation: Operation) -> crate::Result<ExecResponse> {
        self.pooled().exec_operation(operation).await
    }

    pub(crate) async fn exec_raw_sql(&self, raw: RawSql) -> crate::Result<ExecResponse> {
        let span = crate::instrument::raw_sql_span();
        self.pooled().exec_raw_sql(raw, span).await
    }

    /// Begin a transaction on this connection.
    ///
    /// Takes `&mut self` so the `Connection` is exclusively borrowed while
    /// the transaction is open. This prevents statements from running on the
    /// connection directly — bypassing the transaction — when they should
    /// have gone through `&mut tx`.
    pub async fn transaction(&mut self) -> crate::Result<super::Transaction<'_>> {
        <Self as super::Executor>::transaction(self).await
    }

    /// Returns a [`TransactionBuilder`](super::TransactionBuilder) that will
    /// use this connection.
    ///
    /// Like [`transaction`](Self::transaction), this takes `&mut self` so the
    /// `Connection` stays locked for the lifetime of the transaction.
    pub fn transaction_builder(&mut self) -> super::TransactionBuilder<'_> {
        super::TransactionBuilder::new(super::tx::TxSource::Connection(self))
    }

    /// Creates tables and indices defined in the schema on the database.
    pub async fn push_schema(&self) -> crate::Result<()> {
        tracing::info!("pushing schema to database");
        self.pooled().push_schema().await
    }

    #[cfg(feature = "migration")]
    pub(crate) async fn applied_migrations(&self) -> crate::Result<Vec<AppliedMigration>> {
        self.pooled().applied_migrations().await
    }

    #[cfg(feature = "migration")]
    pub(crate) async fn apply_migration(
        &self,
        id: u64,
        name: &str,
        migration: Migration,
    ) -> crate::Result<()> {
        self.pooled()
            .apply_migration(id, name.to_owned(), migration)
            .await
    }
}

#[async_trait]
impl super::Executor for Connection {
    async fn transaction(&mut self) -> crate::Result<Transaction<'_>> {
        Transaction::begin(ConnRef::Borrowed(self)).await
    }

    async fn exec_untyped(
        &mut self,
        stmt: toasty_core::stmt::Statement,
    ) -> crate::Result<ExecResponse> {
        self.exec_stmt(stmt, false).await
    }

    async fn exec_raw_sql(&mut self, raw: RawSql) -> crate::Result<ExecResponse> {
        Connection::exec_raw_sql(self, raw).await
    }

    fn capability(&mut self) -> &Capability {
        self.shared.engine.capability()
    }

    fn schema(&mut self) -> &Arc<Schema> {
        Connection::schema(self)
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("connection", self.pooled())
            .finish()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // A connection that went bad leaves the pool here instead of
        // waiting in the idle set for `recycle` to find it. The pool's
        // sweep, woken by the same failure, then counts and pings only
        // connections that may still be good.
        if let Some(inner) = self.inner.take_if(|inner| inner.is_invalid()) {
            let _ = deadpool::managed::Object::take(inner);
        }
    }
}
