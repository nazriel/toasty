use std::{
    fmt,
    ops::{Bound, RangeBounds},
    path::Path,
};

use crate::{Db, Error, Result};

use super::{
    DatabaseSchema, History, MigrationFile, MigrationSet, SchemaDifference, SchemaDrift, Snapshot,
};

/// Checks every migration of a project, one step at a time, against the
/// snapshot it was generated with.
///
/// For each migration, step by step, the ladder opens a new database and:
///
/// 1. applies every migration before it;
/// 2. runs the [`fill`](Self::fill) hook, which inserts the rows a database at
///    that point would have (by default, none);
/// 3. applies the migration;
/// 4. reads the database's schema back ([`DatabaseSchema::read`]) and compares
///    it with the migration's snapshot ([`SchemaDrift::between`]);
/// 5. runs the [`check`](Self::check) hook, for assertions about the rows.
///
/// The first step that fails stops the run with an error that names the
/// migration and lists the drift. Use it from a test, so a migration that
/// leaves a different schema than its snapshot claims, or fails on a database
/// that has rows, is caught before it ships.
///
/// Reading the schema back needs a driver that supports it; see
/// [`DatabaseSchema::read`].
///
/// # Examples
///
/// ```no_run
/// # #[cfg(feature = "sqlite")]
/// # async fn example() -> toasty::Result<()> {
/// # #[derive(Debug, toasty::Model)]
/// # struct User {
/// #     #[key]
/// #     id: i64,
/// # }
/// use toasty::migration::{MigrationLadder, MigrationSet};
///
/// static MIGRATIONS: MigrationSet = toasty::embed_migrations!();
///
/// async fn connect() -> toasty::Result<toasty::Db> {
///     toasty::Db::builder()
///         .models(toasty::models!(User))
///         .connect("sqlite::memory:")
///         .await
/// }
///
/// // Every migration, on an empty database.
/// MigrationLadder::load(MIGRATIONS, "toasty")?
///     .run(connect)
///     .await?;
///
/// // The newest three, each on a database with a row in it.
/// let count = MIGRATIONS.migrations().len();
/// MigrationLadder::load(MIGRATIONS, "toasty")?
///     .steps(count - 3..)
///     .fill(async |db, _step| {
///         toasty::sql::statement("INSERT INTO users (id) VALUES (1)")
///             .exec(db)
///             .await?;
///         Ok(())
///     })
///     .check(async |db, step| {
///         let rows = toasty::sql::query("SELECT id FROM users").exec(db).await?;
///         assert_eq!(rows.len(), 1, "{} lost the row", step.migration().name());
///         Ok(())
///     })
///     .run(connect)
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct MigrationLadder<F = NoHook, K = NoHook> {
    migrations: MigrationSet,
    snapshots: Vec<Snapshot>,
    start: Bound<usize>,
    end: Bound<usize>,
    allowed: Vec<Allow>,
    fill: F,
    check: K,
}

type Allow = Box<dyn Fn(&SchemaDifference) -> bool>;

/// One step of a [`MigrationLadder`]: the migration it applies, on a database
/// that has every migration before it.
#[derive(Debug, Clone, Copy)]
pub struct Step<'a> {
    index: usize,
    migrations: &'a [MigrationFile],
}

/// The hook a [`MigrationLadder`] runs when none is given. It does nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoHook;

impl MigrationLadder {
    /// Creates a ladder over `migrations`, with `snapshots` holding the
    /// snapshot of each migration, in the same order.
    ///
    /// # Errors
    ///
    /// Returns an error when the number of snapshots differs from the number
    /// of migrations.
    pub fn new(
        migrations: MigrationSet,
        snapshots: impl IntoIterator<Item = Snapshot>,
    ) -> Result<Self> {
        let snapshots: Vec<_> = snapshots.into_iter().collect();
        if snapshots.len() != migrations.migrations().len() {
            return Err(Error::from_args(format_args!(
                "a migration ladder needs one snapshot per migration: {} migrations, {} snapshots",
                migrations.migrations().len(),
                snapshots.len()
            )));
        }
        Ok(Self {
            migrations,
            snapshots,
            start: Bound::Unbounded,
            end: Bound::Unbounded,
            allowed: Vec::new(),
            fill: NoHook,
            check: NoHook,
        })
    }

    /// Creates a ladder over `migrations`, reading each migration's snapshot
    /// from the migration directory `dir`.
    ///
    /// `dir` is the directory the CLI writes to and
    /// [`embed_migrations!`](crate::embed_migrations) reads: it holds
    /// `history.toml` and `snapshots/`. A relative path is relative to the
    /// current directory, which `cargo test` sets to the crate's root.
    /// Each migration's snapshot is the one its history entry names.
    ///
    /// # Errors
    ///
    /// Returns an error when the history cannot be read, a migration has no
    /// entry in it, or a snapshot cannot be loaded.
    pub fn load(migrations: MigrationSet, dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let history_path = dir.join("history.toml");
        let history = History::load(&history_path).map_err(|err| {
            err.context(Error::from_args(format_args!(
                "failed to read {}",
                history_path.display()
            )))
        })?;

        let mut snapshots = Vec::with_capacity(migrations.migrations().len());
        for migration in migrations.migrations() {
            let Some(entry) = history
                .entries()
                .iter()
                .find(|entry| entry.id == migration.id())
            else {
                return Err(Error::from_args(format_args!(
                    "migration {} (id {}) has no entry in {}",
                    migration.name(),
                    migration.id(),
                    history_path.display()
                )));
            };
            let path = dir.join("snapshots").join(&entry.snapshot_name);
            snapshots.push(Snapshot::load(&path).map_err(|err| {
                err.context(Error::from_args(format_args!(
                    "failed to load snapshot {}",
                    path.display()
                )))
            })?);
        }

        Self::new(migrations, snapshots)
    }
}

impl<F, K> MigrationLadder<F, K> {
    /// Runs only the steps whose migration index is in `range`.
    ///
    /// Step `n` applies migration `n` (counting from zero, in application
    /// order) on a database that has migrations `0..n`. Step `0` applies the
    /// first migration on an empty database. The default is every step.
    pub fn steps(mut self, range: impl RangeBounds<usize>) -> Self {
        self.start = range.start_bound().cloned();
        self.end = range.end_bound().cloned();
        self
    }

    /// Accepts every difference `allow` returns `true` for, at every step.
    ///
    /// For what a hand-written migration adds on purpose and the models
    /// cannot declare, such as a partial index or a virtual table. Each call
    /// adds a predicate; a difference any of them accepts does not fail a step.
    ///
    /// ```
    /// # fn example(ladder: toasty::migration::MigrationLadder) {
    /// use toasty::migration::SchemaDifference;
    ///
    /// let ladder = ladder.allow(|difference| {
    ///     matches!(
    ///         difference,
    ///         SchemaDifference::UnexpectedIndex { table, index }
    ///             if table == "users" && index.name == "users_active_email"
    ///     )
    /// });
    /// # }
    /// ```
    pub fn allow(mut self, allow: impl Fn(&SchemaDifference) -> bool + 'static) -> Self {
        self.allowed.push(Box::new(allow));
        self
    }

    /// Sets the hook that fills the database before each step's migration.
    ///
    /// The hook receives the database, with every migration before the step
    /// applied, and the [`Step`]. [`Step::previous`] names the newest migration
    /// the database has. Insert the rows a database at that point would hold,
    /// for example with [`sql::statement`](crate::sql::statement).
    pub fn fill<G>(self, fill: G) -> MigrationLadder<G, K>
    where
        G: AsyncFn(&mut Db, Step<'_>) -> Result<()>,
    {
        MigrationLadder {
            migrations: self.migrations,
            snapshots: self.snapshots,
            start: self.start,
            end: self.end,
            allowed: self.allowed,
            fill,
            check: self.check,
        }
    }

    /// Sets the hook that runs after each step's migration has been applied
    /// and its schema matched the snapshot.
    ///
    /// The hook receives the database and the [`Step`]. Assert what the
    /// migration should have done to the rows the fill hook inserted; return
    /// an error, or panic, to fail the step.
    pub fn check<G>(self, check: G) -> MigrationLadder<F, G>
    where
        G: AsyncFn(&mut Db, Step<'_>) -> Result<()>,
    {
        MigrationLadder {
            migrations: self.migrations,
            snapshots: self.snapshots,
            start: self.start,
            end: self.end,
            allowed: self.allowed,
            fill: self.fill,
            check,
        }
    }
}

impl<F: Hook, K: Hook> MigrationLadder<F, K> {
    /// Runs the selected steps, each on a new database `connect` opens.
    ///
    /// `connect` must return an empty database, such as SQLite's
    /// `sqlite::memory:`, connected with the models the hooks use.
    ///
    /// # Errors
    ///
    /// Stops at the first step that fails and returns an error naming its
    /// migration: when `connect` fails or returns a database that already has
    /// migrations, when a migration or a hook fails, or when the schema after
    /// the migration differs from its snapshot. In the last case the error
    /// lists every difference.
    pub async fn run(&self, connect: impl AsyncFn() -> Result<Db>) -> Result<()> {
        let files = self.migrations.files();
        for index in 0..files.len() {
            if !(self.start, self.end).contains(&index) {
                continue;
            }
            let step = Step {
                index,
                migrations: files,
            };
            self.run_step(step, &connect).await?;
        }
        Ok(())
    }

    async fn run_step(&self, step: Step<'_>, connect: &impl AsyncFn() -> Result<Db>) -> Result<()> {
        let fail = |what: &str| {
            Error::from_args(format_args!(
                "migration {} (step {}): {what}",
                step.migration().name(),
                step.index()
            ))
        };

        let mut db = connect()
            .await
            .map_err(|err| err.context(fail("failed to open a database")))?;

        let before = MigrationSet::new(&self.migrations.files()[..step.index()]);
        let report = before
            .apply(&db)
            .await
            .map_err(|err| err.context(fail("failed to apply the migrations before it")))?;
        if report.skipped() > 0 {
            return Err(fail(
                "the database `connect` returned already has migrations; it must be empty",
            ));
        }

        self.fill
            .call(&mut db, step)
            .await
            .map_err(|err| err.context(fail("the fill hook failed")))?;

        let through = MigrationSet::new(&self.migrations.files()[..=step.index()]);
        let report = through
            .apply(&db)
            .await
            .map_err(|err| err.context(fail("failed to apply")))?;
        if report.applied() != 1 {
            return Err(fail(
                "the database `connect` returned already has this migration; it must be empty",
            ));
        }

        let actual = DatabaseSchema::read(&db)
            .await
            .map_err(|err| err.context(fail("failed to read the database schema")))?;
        let mut drift = SchemaDrift::between(&self.snapshots[step.index()].schema, &actual);
        drift
            .differences
            .retain(|difference| !self.allowed.iter().any(|allow| allow(difference)));
        if !drift.is_empty() {
            let mut message = String::from("the database differs from its snapshot:");
            for difference in &drift.differences {
                message.push_str("\n  - ");
                message.push_str(&difference.to_string());
            }
            return Err(fail(&message));
        }

        self.check
            .call(&mut db, step)
            .await
            .map_err(|err| err.context(fail("the check hook failed")))?;
        Ok(())
    }
}

impl<F, K> fmt::Debug for MigrationLadder<F, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MigrationLadder")
            .field("migrations", &self.migrations)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("allowed", &self.allowed.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Step<'a> {
    /// Returns the index of the migration this step applies, counting from
    /// zero in application order.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns the migration this step applies.
    pub fn migration(&self) -> &'a MigrationFile {
        &self.migrations[self.index]
    }

    /// Returns the newest migration the database has before this step, or
    /// `None` for the first step.
    pub fn previous(&self) -> Option<&'a MigrationFile> {
        self.index
            .checked_sub(1)
            .map(|index| &self.migrations[index])
    }
}

mod sealed {
    use crate::{Db, Result};

    use super::{NoHook, Step};

    /// A fill or check hook of a [`MigrationLadder`](super::MigrationLadder).
    pub trait Hook {
        fn call(&self, db: &mut Db, step: Step<'_>) -> impl Future<Output = Result<()>>;
    }

    impl Hook for NoHook {
        async fn call(&self, _db: &mut Db, _step: Step<'_>) -> Result<()> {
            Ok(())
        }
    }

    impl<F> Hook for F
    where
        F: AsyncFn(&mut Db, Step<'_>) -> Result<()>,
    {
        async fn call(&self, db: &mut Db, step: Step<'_>) -> Result<()> {
            self(db, step).await
        }
    }
}

use sealed::Hook;
