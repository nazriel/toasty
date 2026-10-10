use super::{IntoStatement, Statement};
use crate::{
    Executor, Result,
    schema::{Load, Model},
};
use std::marker::PhantomData;
use toasty_core::stmt;

/// A typed delete statement.
///
/// `Delete<T>` removes records that match the selection built by the
/// originating [`Query`]. Obtain one by calling [`Query::delete`].
///
/// The type parameter `T` is the **returning type**, not the model being
/// deleted. `Query::delete` produces `Delete<()>`, whose
/// [`exec`](Delete::exec) does not return the removed records.
///
/// # Execution
///
/// Call [`exec`](Delete::exec) to run the delete, or convert it into a
/// [`Statement`] with [`IntoStatement`] for batch execution. To learn what
/// the delete removed, call [`exec_count`](Delete::exec_count) for the number
/// of removed records, or [`exec_returning`](Delete::exec_returning) for the
/// records themselves.
///
/// ```
/// # tokio::runtime::Runtime::new().unwrap().block_on(async {
/// # #[derive(Debug, toasty::Model)]
/// # struct User {
/// #     #[key]
/// #     id: i64,
/// #     name: String,
/// # }
/// # let driver = toasty_driver_sqlite::Sqlite::in_memory();
/// # let mut db = toasty::Db::builder().models(toasty::models!(User)).build(driver).await.unwrap();
/// # db.push_schema().await.unwrap();
/// use toasty::stmt::{List, Query};
///
/// Query::<List<User>>::all().filter(User::fields().id().eq(1))
///     .delete()
///     .exec(&mut db)
///     .await
///     .unwrap();
/// # });
/// ```
pub struct Delete<T> {
    pub(crate) untyped: stmt::Delete,
    _p: PhantomData<T>,
}

impl<T> Delete<T> {
    /// Wrap a raw untyped [`stmt::Delete`](toasty_core::stmt::Delete).
    ///
    /// # Examples
    ///
    /// ```
    /// # #[derive(Debug, toasty::Model)]
    /// # struct User {
    /// #     #[key]
    /// #     id: i64,
    /// #     name: String,
    /// # }
    /// use toasty::stmt::{Delete, List, Query};
    ///
    /// // Build a delete from a query
    /// let delete: Delete<()> = Query::<List<User>>::all().delete();
    /// ```
    pub const fn from_untyped(untyped: stmt::Delete) -> Self {
        Self {
            untyped,
            _p: PhantomData,
        }
    }
}

impl<T> Delete<T> {
    /// Attach a condition to this delete. The condition is evaluated after the
    /// filter; if it fails, the operation returns an error (unlike a filter
    /// failure, which silently produces count 0).
    pub fn set_condition(mut self, condition: toasty_core::stmt::Condition) -> Self {
        self.untyped.condition = condition;
        self
    }
}

impl<T> Delete<T> {
    /// Execute this delete and return the number of records it removed.
    ///
    /// The count is what the delete removed, not what matched before it ran:
    /// inside a transaction it reflects the transaction's view, and a filter
    /// that matches nothing returns `0`. Works on every driver.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # #[derive(Debug, toasty::Model)]
    /// # struct User {
    /// #     #[key]
    /// #     id: i64,
    /// #     name: String,
    /// # }
    /// # let driver = toasty_driver_sqlite::Sqlite::in_memory();
    /// # let mut db = toasty::Db::builder().models(toasty::models!(User)).build(driver).await.unwrap();
    /// # db.push_schema().await.unwrap();
    /// # User::create().id(1).name("Alice").exec(&mut db).await.unwrap();
    /// # User::create().id(2).name("Alice").exec(&mut db).await.unwrap();
    /// let removed = User::filter(User::fields().name().eq("Alice"))
    ///     .delete()
    ///     .exec_count(&mut db)
    ///     .await
    ///     .unwrap();
    /// assert_eq!(removed, 2);
    /// # });
    /// ```
    pub async fn exec_count(self, executor: &mut dyn Executor) -> Result<u64> {
        super::exec_count(executor, self.untyped.into()).await
    }

    /// Execute this delete and return the records it removed, with the values
    /// they had when they were removed.
    ///
    /// `M` is the model the delete removes records of; it is usually inferred
    /// from the binding. Naming another model returns an `invalid_statement`
    /// error.
    ///
    /// This needs `DELETE ... RETURNING`. SQLite and PostgreSQL support it;
    /// on MySQL, MariaDB and DynamoDB the delete fails with an
    /// `unsupported_feature` error and removes nothing. Use
    /// [`exec_count`](Delete::exec_count) where only the number is needed.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # #[derive(Debug, toasty::Model)]
    /// # struct User {
    /// #     #[key]
    /// #     id: i64,
    /// #     name: String,
    /// # }
    /// # let driver = toasty_driver_sqlite::Sqlite::in_memory();
    /// # let mut db = toasty::Db::builder().models(toasty::models!(User)).build(driver).await.unwrap();
    /// # db.push_schema().await.unwrap();
    /// # User::create().id(1).name("Alice").exec(&mut db).await.unwrap();
    /// let removed: Vec<User> = User::filter(User::fields().id().eq(1))
    ///     .delete()
    ///     .exec_returning(&mut db)
    ///     .await
    ///     .unwrap();
    /// assert_eq!(removed[0].name, "Alice");
    /// # });
    /// ```
    pub async fn exec_returning<M: Model>(self, executor: &mut dyn Executor) -> Result<Vec<M>> {
        if self.untyped.from.model_id() != Some(M::id()) {
            return Err(toasty_core::Error::invalid_statement(format!(
                "the delete does not remove `{}` records",
                std::any::type_name::<M>()
            )));
        }

        super::exec_returning(executor, self.untyped.into()).await
    }
}

impl<T: Load> Delete<T> {
    /// Execute this delete statement against the given executor.
    ///
    /// Returns `Ok(())` on success. Any matching records are removed from the
    /// database.
    ///
    /// # Examples
    ///
    /// ```
    /// # tokio::runtime::Runtime::new().unwrap().block_on(async {
    /// # #[derive(Debug, toasty::Model)]
    /// # struct User {
    /// #     #[key]
    /// #     id: i64,
    /// #     name: String,
    /// # }
    /// # let driver = toasty_driver_sqlite::Sqlite::in_memory();
    /// # let mut db = toasty::Db::builder().models(toasty::models!(User)).build(driver).await.unwrap();
    /// # db.push_schema().await.unwrap();
    /// User::filter(User::fields().id().eq(1))
    ///     .delete()
    ///     .exec(&mut db)
    ///     .await
    ///     .unwrap();
    /// # });
    /// ```
    pub async fn exec(self, executor: &mut dyn Executor) -> Result<T::Output> {
        executor.exec(self.into()).await
    }
}

impl<T> IntoStatement for Delete<T> {
    type Returning = T;

    fn into_statement(self) -> Statement<T> {
        Statement {
            untyped: self.untyped.into(),
            _p: PhantomData,
        }
    }
}

impl<T> From<Delete<T>> for Statement<T> {
    fn from(value: Delete<T>) -> Self {
        Self {
            untyped: value.untyped.into(),
            _p: PhantomData,
        }
    }
}
