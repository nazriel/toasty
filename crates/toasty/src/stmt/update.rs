use super::{List, Query};
use crate::{
    Executor, Result,
    schema::{Load, Model},
};
use std::{fmt, marker::PhantomData};
use toasty_core::stmt;

/// A typed update statement.
///
/// `Update` modifies records matching a selection, typically derived from a
/// [`Query`]. Field assignments can be added with [`set`](Update::set) or
/// populated by generated update builders.
///
/// The type parameter `T` is the returning type—it determines what `exec()`
/// produces, not which model is being updated. For example, `Update<User>`
/// returns one updated `User`, while `Update<List<User>>` returns the updated
/// records as `Vec<User>`.
///
/// Generated update-builders wrap this type and expose typed setter methods.
/// You rarely construct `Update` by hand.
pub struct Update<T> {
    pub(crate) untyped: stmt::Update,
    _p: PhantomData<T>,
}

// Methods available on all Update<M> regardless of M
impl<T> Update<T> {
    /// Create a new update statement from the given query selection.
    ///
    /// All records matched by `selection` will be updated. By default the
    /// update returns the changed records (`Returning::Changed`).
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// ```
    pub fn new(selection: Query<T>) -> Self {
        let mut stmt = selection.untyped.update();
        stmt.returning = Some(stmt::Returning::Changed);

        Self {
            untyped: stmt,
            _p: PhantomData,
        }
    }

    /// Get a mutable reference to the underlying untyped update.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// let raw = update.as_untyped_mut();
    /// // Inspect or modify the raw update
    /// assert!(raw.returning.is_some());
    /// ```
    pub fn as_untyped_mut(&mut self) -> &mut stmt::Update {
        &mut self.untyped
    }

    /// Replace all field assignments with `assignments`.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// update.set_assignments(toasty_core::stmt::Assignments::default());
    /// ```
    pub fn set_assignments(&mut self, assignments: stmt::Assignments) {
        self.untyped.assignments = assignments;
    }

    /// Assign a value to a field.
    ///
    /// `field` identifies which field to update and `expr` is the new value.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// update.set(1, toasty_core::stmt::Value::from("Bob"));
    /// ```
    pub fn set(&mut self, field: impl Into<stmt::Projection>, expr: impl Into<stmt::Expr>) {
        self.untyped.assignments.set(field, expr);
    }

    /// Consume this typed update and return the untyped core statement.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// let _raw = update.into_untyped();
    /// ```
    pub fn into_untyped(self) -> stmt::Statement {
        self.untyped.into()
    }
}

impl<T: Load> Update<T> {
    /// Execute this update statement against the given executor.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// update.set(1, toasty_core::stmt::Value::from("Bob"));
    /// let _users: Vec<User> = update.exec(&mut db).await.unwrap();
    /// # });
    /// ```
    pub async fn exec(self, executor: &mut dyn Executor) -> Result<T::Output> {
        executor.exec(self.into()).await
    }
}

impl<T> Update<T> {
    /// Execute this update and return the number of records it changed.
    ///
    /// The count is what the update changed, not what matched before it ran:
    /// inside a transaction it reflects the transaction's view, and a filter
    /// that matches nothing returns `0`. A record the update matched counts
    /// even when its assigned values equal the stored ones. Works on every
    /// driver.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// update.set(1, toasty_core::stmt::Value::from("Bob"));
    /// assert_eq!(update.exec_count(&mut db).await.unwrap(), 1);
    /// # });
    /// ```
    pub async fn exec_count(self, executor: &mut dyn Executor) -> Result<u64> {
        super::exec_count(executor, self.untyped.into()).await
    }
}

impl<M: Model> Update<List<M>> {
    /// Execute this update and return the records it changed, with their
    /// values after the update.
    ///
    /// A filter that matches nothing returns an empty `Vec`, so an update
    /// filtered on a field's expected value doubles as a compare-and-set: an
    /// empty result means another writer changed the record first.
    ///
    /// SQLite and PostgreSQL return the rows with `UPDATE ... RETURNING`.
    /// MySQL and MariaDB have no `RETURNING` on `UPDATE`; there the records
    /// are read back with a `SELECT` over the update's filter after the update
    /// runs. That read is not atomic with the update, and it misses a record
    /// the update moved out of its own filter, so a compare-and-set needs
    /// [`exec_count`](Update::exec_count) on those drivers.
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
    /// use toasty::stmt::{List, Query, Update};
    ///
    /// let mut update = Update::<List<User>>::new(Query::<List<User>>::all());
    /// update.set(1, toasty_core::stmt::Value::from("Bob"));
    /// let users: Vec<User> = update.exec_returning(&mut db).await.unwrap();
    /// assert_eq!(users[0].name, "Bob");
    /// # });
    /// ```
    pub async fn exec_returning(self, executor: &mut dyn Executor) -> Result<Vec<M>> {
        super::exec_returning(executor, self.untyped.into()).await
    }
}

impl<M> Clone for Update<M> {
    fn clone(&self) -> Self {
        Self {
            untyped: self.untyped.clone(),
            _p: PhantomData,
        }
    }
}

impl<M: Model> Default for Update<M> {
    fn default() -> Self {
        Self {
            untyped: stmt::Update {
                target: stmt::UpdateTarget::Model(M::id()),
                assignments: stmt::Assignments::default(),
                filter: stmt::Filter::new(stmt::Expr::from(false)),
                condition: stmt::Condition::default(),
                returning: Some(stmt::Returning::Changed),
            },
            _p: PhantomData,
        }
    }
}

impl<M> AsRef<stmt::Update> for Update<M> {
    fn as_ref(&self) -> &stmt::Update {
        &self.untyped
    }
}

impl<M> AsMut<stmt::Update> for Update<M> {
    fn as_mut(&mut self) -> &mut stmt::Update {
        &mut self.untyped
    }
}

impl<M> fmt::Debug for Update<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.untyped.fmt(f)
    }
}
