#![cfg(feature = "migration")]

use std::cell::RefCell;

use toasty::Db;
use toasty::migration::{
    DatabaseSchema, IndexShape, MigrationFile, MigrationLadder, MigrationSet, SchemaDifference,
    SchemaDrift, Snapshot,
};
use toasty::schema::{db, diff};
use toasty_driver_sqlite::Sqlite;

/// The first version: users with a unique email and an indexed name.
mod v1 {
    #[derive(Debug, toasty::Model)]
    #[table = "users"]
    pub struct User {
        #[key]
        pub id: i64,
        #[unique]
        pub email: String,
        #[index]
        pub name: String,
    }
}

/// `name` becomes nullable, which SQLite can only do by rebuilding the table.
/// The unique index on `email` does not change.
mod v2 {
    #[derive(Debug, toasty::Model)]
    #[table = "users"]
    pub struct User {
        #[key]
        pub id: i64,
        #[unique]
        pub email: String,
        #[index]
        pub name: Option<String>,
    }
}

/// A second table joins.
mod v3 {
    #[derive(Debug, toasty::Model)]
    #[table = "users"]
    pub struct User {
        #[key]
        pub id: i64,
        #[unique]
        pub email: String,
        #[index]
        pub name: Option<String>,
    }

    #[derive(Debug, toasty::Model)]
    #[table = "posts"]
    pub struct Post {
        #[key]
        pub id: i64,
        pub title: String,
    }
}

async fn schema_of(models: toasty::ModelSet) -> db::Schema {
    let db = Db::builder()
        .models(models)
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.schema().db.clone()
}

async fn connect() -> toasty::Result<Db> {
    Db::builder()
        .models(toasty::models!(v3::User, v3::Post))
        .build(Sqlite::in_memory())
        .await
}

/// The three versions' snapshots, and the migrations this generator writes
/// from one to the next.
async fn history() -> (Vec<String>, Vec<Snapshot>) {
    let schemas = [
        db::Schema::default(),
        schema_of(toasty::models!(v1::User)).await,
        schema_of(toasty::models!(v2::User)).await,
        schema_of(toasty::models!(v3::User, v3::Post)).await,
    ];
    let driver = Sqlite::in_memory();
    let mut migrations = Vec::new();
    let mut snapshots = Vec::new();
    for pair in schemas.windows(2) {
        let generated =
            toasty::migration::generate(&driver, &pair[0], &pair[1], &diff::RenameHints::new())
                .unwrap();
        let db::Migration::Sql(sql) = generated.migration;
        migrations.push(sql);
        snapshots.push(generated.snapshot);
    }
    (migrations, snapshots)
}

/// A migration set over `sql`, named `0000_*.sql` onwards. The SQL is leaked:
/// a set holds `'static` files, as `embed_migrations!` builds them.
fn migration_set(sql: Vec<String>) -> MigrationSet {
    let names = ["0000_users.sql", "0001_nullable_name.sql", "0002_posts.sql"];
    let files: Vec<MigrationFile> = sql
        .into_iter()
        .enumerate()
        .map(|(i, sql)| MigrationFile::new(i as u64 + 1, names[i], Box::leak(sql.into_boxed_str())))
        .collect();
    MigrationSet::new(Box::leak(files.into_boxed_slice()))
}

#[tokio::test]
async fn ladder_passes_a_history_whose_rebuild_keeps_its_unique_index() {
    let (migrations, snapshots) = history().await;
    assert!(
        migrations[1].contains("\"_toasty_new_users\""),
        "the second migration rebuilds the table:\n{}",
        migrations[1]
    );

    MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .run(connect)
        .await
        .unwrap();
}

#[tokio::test]
async fn ladder_fails_at_the_step_whose_rebuild_loses_an_index_and_names_it() {
    let (mut migrations, snapshots) = history().await;
    let create = migrations[1]
        .lines()
        .find(|line| line.starts_with("CREATE UNIQUE INDEX"))
        .expect("the rebuild recreates the unique index")
        .to_string();
    migrations[1] = migrations[1].replace(&create, "");

    let err = MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .run(connect)
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "migration 0001_nullable_name.sql (step 1): the database differs from its snapshot:\n  \
         - index \"users\".\"index_users_by_email\" is missing (unique on \"email\")"
    );
}

#[tokio::test]
async fn ladder_runs_the_selected_steps_with_the_fill_and_check_hooks() {
    let (migrations, snapshots) = history().await;
    let seen = RefCell::new(Vec::new());

    MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .steps(1..)
        .fill(async |db, step| {
            assert_eq!(step.previous().unwrap().id(), step.index() as u64);
            toasty::sql::statement(
                "INSERT INTO users (id, email, name) VALUES (1, 'alice@example.com', 'Alice')",
            )
            .exec(db)
            .await?;
            Ok(())
        })
        .check(async |db, step| {
            let rows = toasty::sql::query("SELECT email, name FROM users")
                .exec(db)
                .await?;
            seen.borrow_mut()
                .push((step.migration().name().to_string(), rows.len()));
            Ok(())
        })
        .run(connect)
        .await
        .unwrap();

    assert_eq!(
        seen.into_inner(),
        [
            ("0001_nullable_name.sql".to_string(), 1),
            ("0002_posts.sql".to_string(), 1),
        ]
    );
}

#[tokio::test]
async fn ladder_reports_a_failing_check_hook_with_its_migration() {
    let (migrations, snapshots) = history().await;

    let err = MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .steps(2..=2)
        .check(async |_db, _step| Err(toasty::Error::from_args(format_args!("no posts"))))
        .run(connect)
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "migration 0002_posts.sql (step 2): the check hook failed: no posts"
    );
}

#[tokio::test]
async fn ladder_needs_one_snapshot_per_migration() {
    let (migrations, mut snapshots) = history().await;
    snapshots.pop();

    let err = MigrationLadder::new(migration_set(migrations), snapshots).unwrap_err();

    assert_eq!(
        err.to_string(),
        "a migration ladder needs one snapshot per migration: 3 migrations, 2 snapshots"
    );
}

#[tokio::test]
async fn ladder_loads_the_snapshots_a_history_file_names() {
    let (migrations, snapshots) = history().await;
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("snapshots")).unwrap();
    let mut history = toasty::migration::History::new();
    for (i, snapshot) in snapshots.iter().enumerate() {
        let snapshot_name = format!("{i:04}_snapshot.toml");
        snapshot
            .save(dir.path().join("snapshots").join(&snapshot_name))
            .unwrap();
        history.add_entry(toasty::migration::HistoryEntry {
            id: i as u64 + 1,
            name: format!("{i:04}.sql"),
            snapshot_name,
            checksum: None,
        });
    }
    history.save(dir.path().join("history.toml")).unwrap();

    MigrationLadder::load(migration_set(migrations), dir.path())
        .unwrap()
        .run(connect)
        .await
        .unwrap();
}

/// The first version's schema in a database whose indices were then changed
/// by hand.
async fn drifted_database() -> (db::Schema, DatabaseSchema) {
    let mut db = Db::builder()
        .models(toasty::models!(v1::User))
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();
    for sql in [
        "DROP INDEX index_users_by_name",
        "DROP INDEX index_users_by_email",
        "CREATE INDEX index_users_by_email ON users (email, name)",
        "CREATE INDEX index_users_by_id_and_name ON users (id, name)",
        "ALTER TABLE users ADD COLUMN nickname TEXT",
        "CREATE TABLE audit (id INTEGER PRIMARY KEY)",
    ] {
        toasty::sql::statement(sql).exec(&mut db).await.unwrap();
    }
    let actual = DatabaseSchema::read(&db).await.unwrap();
    (db.schema().db.clone(), actual)
}

#[tokio::test]
async fn drift_is_empty_for_a_database_that_matches() {
    let db = Db::builder()
        .models(toasty::models!(v3::User, v3::Post))
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    let actual = DatabaseSchema::read(&db).await.unwrap();

    assert_eq!(
        actual
            .tables
            .iter()
            .map(|table| table.name.as_str())
            .collect::<Vec<_>>(),
        ["posts", "users"]
    );
    let drift = SchemaDrift::between(&db.schema().db, &actual);
    assert!(drift.is_empty(), "{drift}");
}

#[tokio::test]
async fn drift_lists_missing_unexpected_and_changed_indices() {
    let (expected, actual) = drifted_database().await;

    let drift = SchemaDrift::between(&expected, &actual);

    let shape = |name: &str, unique: bool, columns: &[&str]| IndexShape {
        name: name.to_string(),
        unique,
        columns: columns.iter().map(|c| Some(c.to_string())).collect(),
        filter: None,
    };
    assert_eq!(
        drift.differences,
        [
            SchemaDifference::UnexpectedColumn {
                table: "users".into(),
                column: "nickname".into(),
            },
            SchemaDifference::ChangedIndex {
                table: "users".into(),
                expected: shape("index_users_by_email", true, &["email"]),
                actual: shape("index_users_by_email", false, &["email", "name"]),
            },
            SchemaDifference::MissingIndex {
                table: "users".into(),
                index: shape("index_users_by_name", false, &["name"]),
            },
            SchemaDifference::UnexpectedIndex {
                table: "users".into(),
                index: shape("index_users_by_id_and_name", false, &["id", "name"]),
            },
            SchemaDifference::UnexpectedTable {
                table: "audit".into(),
            },
        ]
    );
    assert_eq!(
        drift.to_string(),
        "column \"users\".\"nickname\" is not in the expected schema\n\
         index \"users\".\"index_users_by_email\" is on \"email\", \"name\" but expected unique on \"email\"\n\
         index \"users\".\"index_users_by_name\" is missing (on \"name\")\n\
         index \"users\".\"index_users_by_id_and_name\" is not in the expected schema (on \"id\", \"name\")\n\
         table \"audit\" is not in the expected schema\n"
    );
}

#[tokio::test]
async fn drift_reports_missing_tables_and_columns_and_nullability() {
    let mut expected = schema_of(toasty::models!(v3::User, v3::Post)).await;
    let actual = {
        let db = Db::builder()
            .models(toasty::models!(v1::User))
            .build(Sqlite::in_memory())
            .await
            .unwrap();
        db.push_schema().await.unwrap();
        DatabaseSchema::read(&db).await.unwrap()
    };
    expected.tables[0]
        .columns
        .retain(|column| column.name != "name");
    let mut extra = expected.tables[0].columns[0].clone();
    extra.name = "nickname".into();
    expected.tables[0].columns.push(extra);
    expected.tables[0]
        .indices
        .retain(|index| index.name != "index_users_by_name");

    let drift = SchemaDrift::between(&expected, &actual);

    let users = actual.table("users").unwrap();
    assert!(!users.column("name").unwrap().nullable);
    assert!(drift.differences.contains(&SchemaDifference::MissingTable {
        table: "posts".into()
    }));
    assert!(
        drift
            .differences
            .contains(&SchemaDifference::MissingColumn {
                table: "users".into(),
                column: "nickname".into(),
            })
    );
    assert!(
        drift
            .differences
            .contains(&SchemaDifference::UnexpectedColumn {
                table: "users".into(),
                column: "name".into(),
            })
    );
}

#[tokio::test]
async fn drift_reports_a_column_whose_nullability_differs() {
    let expected = schema_of(toasty::models!(v2::User)).await;
    let actual = {
        let db = Db::builder()
            .models(toasty::models!(v1::User))
            .build(Sqlite::in_memory())
            .await
            .unwrap();
        db.push_schema().await.unwrap();
        DatabaseSchema::read(&db).await.unwrap()
    };

    let drift = SchemaDrift::between(&expected, &actual);

    assert_eq!(
        drift.to_string(),
        "column \"users\".\"name\" is NOT NULL but expected nullable\n"
    );
}

#[tokio::test]
async fn ladder_accepts_the_differences_allow_names_and_no_others() {
    let (mut migrations, snapshots) = history().await;
    migrations[0].push_str(
        "\n-- #[toasty::breakpoint]\nCREATE INDEX users_by_lower_name ON users (lower(name));",
    );
    let set = migration_set(migrations);

    let err = MigrationLadder::new(set, snapshots.clone())
        .unwrap()
        .run(connect)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "migration 0000_users.sql (step 0): the database differs from its snapshot:\n  \
         - index \"users\".\"users_by_lower_name\" is not in the expected schema (on an expression)"
    );

    MigrationLadder::new(set, snapshots)
        .unwrap()
        .allow(|difference| {
            matches!(
                difference,
                SchemaDifference::UnexpectedIndex { table, index }
                    if table == "users" && index.name == "users_by_lower_name"
            )
        })
        .run(connect)
        .await
        .unwrap();
}

#[tokio::test]
async fn drift_compares_an_index_named_like_the_primary_key_when_the_database_has_one() {
    let mut db = Db::builder()
        .models(toasty::models!(v1::User))
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();
    let primary_key = db.schema().db.tables[0]
        .indices
        .iter()
        .find(|index| index.primary_key)
        .unwrap()
        .name
        .clone();

    toasty::sql::statement(format!("CREATE UNIQUE INDEX {primary_key} ON users (id)"))
        .exec(&mut db)
        .await
        .unwrap();
    let actual = DatabaseSchema::read(&db).await.unwrap();
    let drift = SchemaDrift::between(&db.schema().db, &actual);
    assert!(drift.is_empty(), "{drift}");

    toasty::sql::statement(format!("DROP INDEX {primary_key}"))
        .exec(&mut db)
        .await
        .unwrap();
    toasty::sql::statement(format!("CREATE INDEX {primary_key} ON users (id, name)"))
        .exec(&mut db)
        .await
        .unwrap();
    let actual = DatabaseSchema::read(&db).await.unwrap();
    let drift = SchemaDrift::between(&db.schema().db, &actual);
    assert_eq!(
        drift.to_string(),
        format!(
            "index \"users\".\"{primary_key}\" is on \"id\", \"name\" but expected unique on \"id\"\n"
        )
    );
}

#[tokio::test]
async fn ladder_refuses_a_database_that_already_has_migrations() {
    let (migrations, snapshots) = history().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");

    let err = MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .run(async || {
            Db::builder()
                .models(toasty::models!(v3::User, v3::Post))
                .build(Sqlite::open(&path))
                .await
        })
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "migration 0001_nullable_name.sql (step 1): the database `connect` returned already \
         has migrations; it must be empty"
    );
}
