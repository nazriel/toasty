#![cfg(feature = "migration")]

use toasty::Db;
use toasty::migration::{
    DatabaseSchema, IndexShape, MigrationFile, MigrationLadder, MigrationSet, SchemaDifference,
    SchemaDrift, Snapshot,
};
use toasty::schema::{db, diff};
use toasty_driver_sqlite::Sqlite;

/// The first version: a token's name is unique per owner among the tokens
/// that are not revoked.
mod v1 {
    #[derive(Debug, toasty::Model)]
    #[table = "tokens"]
    #[unique(owner, name, filter = "revoked_at IS NULL")]
    pub struct Token {
        #[key]
        pub id: i64,
        pub owner: i64,
        pub name: String,
        pub revoked_at: Option<i64>,
    }
}

/// The predicate changes; nothing else does.
mod v2 {
    #[derive(Debug, toasty::Model)]
    #[table = "tokens"]
    #[unique(owner, name, filter = "revoked_at IS NULL AND owner > 0")]
    pub struct Token {
        #[key]
        pub id: i64,
        pub owner: i64,
        pub name: String,
        pub revoked_at: Option<i64>,
    }
}

/// `name` becomes nullable, which SQLite can only do by rebuilding the table.
/// The partial index does not change.
mod v3 {
    #[derive(Debug, toasty::Model)]
    #[table = "tokens"]
    #[unique(owner, name, filter = "revoked_at IS NULL AND owner > 0")]
    pub struct Token {
        #[key]
        pub id: i64,
        pub owner: i64,
        pub name: Option<String>,
        pub revoked_at: Option<i64>,
    }
}

const INDEX: &str = "index_tokens_by_owner_and_name";

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
        .models(toasty::models!(v3::Token))
        .build(Sqlite::in_memory())
        .await
}

/// The three versions' snapshots, and the migrations the generator writes
/// from one to the next.
async fn history() -> (Vec<String>, Vec<Snapshot>) {
    let schemas = [
        db::Schema::default(),
        schema_of(toasty::models!(v1::Token)).await,
        schema_of(toasty::models!(v2::Token)).await,
        schema_of(toasty::models!(v3::Token)).await,
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

fn migration_set(sql: Vec<String>) -> MigrationSet {
    let names = [
        "0000_tokens.sql",
        "0001_predicate.sql",
        "0002_nullable_name.sql",
    ];
    let files: Vec<MigrationFile> = sql
        .into_iter()
        .enumerate()
        .map(|(i, sql)| MigrationFile::new(i as u64 + 1, names[i], Box::leak(sql.into_boxed_str())))
        .collect();
    MigrationSet::new(Box::leak(files.into_boxed_slice()))
}

fn index_lines(sql: &str) -> Vec<&str> {
    sql.lines().filter(|line| line.contains("INDEX")).collect()
}

#[tokio::test]
async fn the_schema_records_the_declared_predicate() {
    let schema = schema_of(toasty::models!(v1::Token)).await;
    let index = schema.tables[0]
        .indices
        .iter()
        .find(|index| index.name == INDEX)
        .unwrap();

    assert!(index.unique);
    assert_eq!(index.filter.as_deref(), Some("revoked_at IS NULL"));
    assert!(index.is_partial());
    assert!(!index.is_unique_over_all_rows());
}

#[tokio::test]
async fn generate_creates_a_partial_index_with_its_predicate() {
    let (migrations, _) = history().await;

    assert_eq!(
        index_lines(&migrations[0]),
        [
            r#"CREATE UNIQUE INDEX "index_tokens_by_owner_and_name" ON "tokens" ("owner", "name") WHERE revoked_at IS NULL;"#
        ],
    );
}

#[tokio::test]
async fn a_changed_predicate_drops_and_recreates_the_index() {
    let (migrations, _) = history().await;

    assert_eq!(
        index_lines(&migrations[1]),
        [
            r#"DROP INDEX "index_tokens_by_owner_and_name";"#,
            r#"CREATE UNIQUE INDEX "index_tokens_by_owner_and_name" ON "tokens" ("owner", "name") WHERE revoked_at IS NULL AND owner > 0;"#,
        ],
    );
}

#[tokio::test]
async fn a_rebuild_recreates_the_partial_index_with_its_predicate() {
    let (migrations, _) = history().await;

    assert!(
        migrations[2].contains("\"_toasty_new_tokens\""),
        "the third migration rebuilds the table:\n{}",
        migrations[2]
    );
    assert_eq!(
        index_lines(&migrations[2]),
        [
            r#"CREATE UNIQUE INDEX "index_tokens_by_owner_and_name" ON "tokens" ("owner", "name") WHERE revoked_at IS NULL AND owner > 0;"#
        ],
    );
}

#[tokio::test]
async fn an_unchanged_partial_index_generates_no_migration() {
    let schema = schema_of(toasty::models!(v2::Token)).await;

    let generated = toasty::migration::generate(
        &Sqlite::in_memory(),
        &schema,
        &schema,
        &diff::RenameHints::new(),
    );

    assert!(generated.is_none());
}

#[tokio::test]
async fn the_ladder_passes_a_history_with_a_partial_index() {
    let (migrations, snapshots) = history().await;

    MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .run(connect)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_ladder_fails_at_a_step_that_loses_the_predicate() {
    let (mut migrations, snapshots) = history().await;
    migrations[2] = migrations[2].replace(" WHERE revoked_at IS NULL AND owner > 0", "");

    let err = MigrationLadder::new(migration_set(migrations), snapshots)
        .unwrap()
        .run(connect)
        .await
        .unwrap_err();

    assert_eq!(
        err.to_string(),
        "migration 0002_nullable_name.sql (step 2): the database differs from its snapshot:\n  \
         - index \"tokens\".\"index_tokens_by_owner_and_name\" is unique on \"owner\", \"name\" \
         but expected unique on \"owner\", \"name\" where revoked_at IS NULL AND owner > 0"
    );
}

/// The first version's schema in a database whose partial index was then
/// replaced by `create`.
async fn database_with_index(create: &str) -> (db::Schema, DatabaseSchema) {
    let mut db = Db::builder()
        .models(toasty::models!(v1::Token))
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();
    for sql in [&format!("DROP INDEX {INDEX}")[..], create] {
        toasty::sql::statement(sql).exec(&mut db).await.unwrap();
    }
    let actual = DatabaseSchema::read(&db).await.unwrap();
    (db.schema().db.clone(), actual)
}

fn shape(filter: Option<&str>) -> IndexShape {
    IndexShape {
        name: INDEX.to_string(),
        unique: true,
        columns: vec![Some("owner".to_string()), Some("name".to_string())],
        filter: filter.map(str::to_string),
    }
}

#[tokio::test]
async fn read_reports_a_partial_index_predicate_and_drift_is_empty() {
    let db = Db::builder()
        .models(toasty::models!(v1::Token))
        .build(Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    let actual = DatabaseSchema::read(&db).await.unwrap();

    let index = actual.table("tokens").unwrap().index(INDEX).unwrap();
    assert_eq!(index.filter.as_deref(), Some("revoked_at IS NULL"));
    let drift = SchemaDrift::between(&db.schema().db, &actual);
    assert!(drift.is_empty(), "{drift}");
}

#[tokio::test]
async fn drift_reports_an_index_that_lost_its_predicate() {
    let (expected, actual) = database_with_index(&format!(
        "CREATE UNIQUE INDEX {INDEX} ON tokens (owner, name)"
    ))
    .await;

    let drift = SchemaDrift::between(&expected, &actual);

    assert_eq!(
        drift.differences,
        [SchemaDifference::ChangedIndex {
            table: "tokens".into(),
            expected: shape(Some("revoked_at IS NULL")),
            actual: shape(None),
        }]
    );
}

#[tokio::test]
async fn drift_reports_an_index_with_another_predicate() {
    let (expected, actual) = database_with_index(&format!(
        "CREATE UNIQUE INDEX {INDEX} ON tokens (owner, name) WHERE revoked_at IS NOT NULL"
    ))
    .await;

    let drift = SchemaDrift::between(&expected, &actual);

    assert_eq!(
        drift.differences,
        [SchemaDifference::ChangedIndex {
            table: "tokens".into(),
            expected: shape(Some("revoked_at IS NULL")),
            actual: shape(Some("revoked_at IS NOT NULL")),
        }]
    );
    assert_eq!(
        drift.to_string(),
        "index \"tokens\".\"index_tokens_by_owner_and_name\" is unique on \"owner\", \"name\" \
         where revoked_at IS NOT NULL but expected unique on \"owner\", \"name\" where \
         revoked_at IS NULL\n"
    );
}

#[tokio::test]
async fn drift_reports_a_predicate_the_schema_does_not_declare() {
    let (mut expected, actual) = database_with_index(&format!(
        "CREATE UNIQUE INDEX {INDEX} ON tokens (owner, name) WHERE revoked_at IS NULL"
    ))
    .await;
    for index in &mut expected.tables[0].indices {
        index.filter = None;
    }

    let drift = SchemaDrift::between(&expected, &actual);

    assert_eq!(
        drift.differences,
        [SchemaDifference::ChangedIndex {
            table: "tokens".into(),
            expected: shape(None),
            actual: shape(Some("revoked_at IS NULL")),
        }]
    );
}

#[tokio::test]
async fn a_snapshot_round_trips_a_predicate() {
    let schema = schema_of(toasty::models!(v1::Token)).await;
    let toml = Snapshot::new(schema).to_toml_string().unwrap();
    assert!(
        toml.contains(r#"filter = "revoked_at IS NULL""#),
        "snapshot:\n{toml}"
    );

    let loaded: Snapshot = toml.parse().unwrap();

    let index = loaded.schema.tables[0]
        .indices
        .iter()
        .find(|index| index.name == INDEX)
        .unwrap();
    assert_eq!(index.filter.as_deref(), Some("revoked_at IS NULL"));
    assert_eq!(loaded.to_toml_string().unwrap(), toml);
}

#[tokio::test]
async fn a_snapshot_without_a_predicate_has_no_filter_key_and_still_loads() {
    let schema = schema_of(toasty::models!(v1::Token)).await;
    let with_filter = Snapshot::new(schema).to_toml_string().unwrap();
    // A snapshot written before partial indexes existed has no `filter` key.
    let without_filter: String = with_filter
        .lines()
        .filter(|line| !line.starts_with("filter = "))
        .map(|line| format!("{line}\n"))
        .collect();
    assert_ne!(with_filter, without_filter);

    let loaded: Snapshot = without_filter.parse().unwrap();

    assert!(
        loaded.schema.tables[0]
            .indices
            .iter()
            .all(|index| index.filter.is_none())
    );
    assert_eq!(loaded.to_toml_string().unwrap(), without_filter);
}
