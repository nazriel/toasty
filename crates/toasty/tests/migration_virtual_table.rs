#![cfg(all(feature = "migration", feature = "sqlite"))]

use toasty::Db;
use toasty::migration::{
    DatabaseSchema, MigrationFile, MigrationLadder, MigrationSet, SchemaDifference, SchemaDrift,
    SkippedTable, SkippedTableKind, Snapshot,
};
use toasty::schema::{db, diff};
use toasty_driver_sqlite::Sqlite;

#[derive(Debug, toasty::Model)]
#[table = "articles"]
struct Article {
    #[key]
    id: i64,
    title: String,
    body: String,
}

/// A full-text index over `articles`, and the triggers that keep it current.
/// The models cannot declare any of it, so the migration is written by hand.
const SEARCH_INDEX: &str = r#"-- Write this migration's SQL here.
CREATE VIRTUAL TABLE articles_fts USING fts5(title, body, content='articles', content_rowid='id');
-- #[toasty::breakpoint]
CREATE TRIGGER articles_after_insert AFTER INSERT ON articles BEGIN
    INSERT INTO articles_fts(rowid, title, body) VALUES (new.id, new.title, new.body);
END;
-- #[toasty::breakpoint]
CREATE TRIGGER articles_after_delete AFTER DELETE ON articles BEGIN
    INSERT INTO articles_fts(articles_fts, rowid, title, body) VALUES ('delete', old.id, old.title, old.body);
END;
"#;

async fn connect() -> toasty::Result<Db> {
    Db::builder()
        .models(toasty::models!(Article))
        .build(Sqlite::in_memory())
        .await
}

/// The history: the generated migration that creates `articles`, then a
/// hand-written one with the full-text index. Its snapshot equals the one
/// before it, as `migration new` writes it.
async fn history(search_index: &str) -> (MigrationSet, Vec<Snapshot>) {
    let db = connect().await.unwrap();
    let generated = toasty::migration::generate(
        &Sqlite::in_memory(),
        &db::Schema::default(),
        &db.schema().db,
        &diff::RenameHints::new(),
    )
    .unwrap();
    let db::Migration::Sql(create) = generated.migration;

    let files = vec![
        MigrationFile::new(1, "0000_articles.sql", Box::leak(create.into_boxed_str())),
        MigrationFile::new(
            2,
            "0001_search_index.sql",
            Box::leak(search_index.to_string().into_boxed_str()),
        ),
    ];
    let snapshots = vec![generated.snapshot.clone(), generated.snapshot];
    (
        MigrationSet::new(Box::leak(files.into_boxed_slice())),
        snapshots,
    )
}

async fn ids_matching(db: &mut Db, query: &str) -> Vec<i64> {
    toasty::sql::query("SELECT rowid FROM articles_fts WHERE articles_fts MATCH ?1 ORDER BY rowid")
        .bind(query)
        .exec(db)
        .await
        .unwrap()
        .iter()
        .map(|row| match row {
            toasty::stmt::Value::Record(record) => match &record[0] {
                toasty::stmt::Value::I64(id) => *id,
                other => panic!("expected an integer id, got {other:?}"),
            },
            other => panic!("expected a record, got {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn a_hand_written_full_text_index_passes_the_ladder_and_answers_match_queries() {
    let (migrations, snapshots) = history(SEARCH_INDEX).await;

    MigrationLadder::new(migrations, snapshots)
        .unwrap()
        .steps(1..)
        .check(async |db, _step| {
            for (id, title, body) in [
                (1, "Sourdough", "Flour, water and salt."),
                (2, "Rye bread", "Rye flour makes a dense loaf."),
                (3, "Soup", "Stock, onions and salt."),
            ] {
                toasty::sql::statement(
                    "INSERT INTO articles (id, title, body) VALUES (?1, ?2, ?3)",
                )
                .bind(id as i64)
                .bind(title)
                .bind(body)
                .exec(db)
                .await?;
            }

            assert_eq!(ids_matching(db, "flour").await, [1, 2]);
            assert_eq!(ids_matching(db, "salt").await, [1, 3]);
            assert_eq!(ids_matching(db, "title:rye").await, [2]);

            toasty::sql::statement("DELETE FROM articles WHERE id = 2")
                .exec(db)
                .await?;
            assert_eq!(ids_matching(db, "flour").await, [1]);
            Ok(())
        })
        .run(connect)
        .await
        .unwrap();
}

#[tokio::test]
async fn schema_reading_skips_a_virtual_table_and_its_shadow_tables_and_reports_them() {
    let (migrations, snapshots) = history(SEARCH_INDEX).await;
    let mut db = connect().await.unwrap();
    for file in migrations.migrations() {
        for statement in db::Migration::Sql(file.sql().to_string()).statements() {
            toasty::sql::statement(statement)
                .exec(&mut db)
                .await
                .unwrap();
        }
    }

    let schema = DatabaseSchema::read(&db).await.unwrap();

    let tables: Vec<_> = schema.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tables, ["articles"]);
    assert_eq!(
        schema.skipped_table("articles_fts"),
        Some(&SkippedTable {
            name: "articles_fts".into(),
            kind: SkippedTableKind::Virtual,
        })
    );
    let shadows: Vec<_> = schema
        .skipped
        .iter()
        .filter(|table| table.kind == SkippedTableKind::Shadow)
        .map(|table| table.name.as_str())
        .collect();
    assert_eq!(
        shadows,
        [
            "articles_fts_config",
            "articles_fts_data",
            "articles_fts_docsize",
            "articles_fts_idx"
        ]
    );

    let drift = SchemaDrift::between(&snapshots[1].schema, &schema);
    assert!(drift.is_empty(), "{drift}");
}

#[tokio::test]
async fn a_table_named_like_a_shadow_table_is_still_compared() {
    let (migrations, snapshots) = history(SEARCH_INDEX).await;
    let mut db = connect().await.unwrap();
    for file in migrations.migrations() {
        for statement in db::Migration::Sql(file.sql().to_string()).statements() {
            toasty::sql::statement(statement)
                .exec(&mut db)
                .await
                .unwrap();
        }
    }
    // The suffix is what shadow tables end in, but SQLite does not call this
    // one a shadow table, so the schema check must not skip it.
    toasty::sql::statement("CREATE TABLE reports_data (id INTEGER PRIMARY KEY)")
        .exec(&mut db)
        .await
        .unwrap();

    let schema = DatabaseSchema::read(&db).await.unwrap();

    assert!(schema.table("reports_data").is_some());
    assert!(schema.skipped_table("reports_data").is_none());
    let drift = SchemaDrift::between(&snapshots[1].schema, &schema);
    assert_eq!(
        drift.differences,
        [SchemaDifference::UnexpectedTable {
            table: "reports_data".into()
        }]
    );
}
