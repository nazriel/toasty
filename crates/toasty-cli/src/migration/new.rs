use super::generate::write_migration;
use crate::Config;
use anyhow::{Context, Result};
use clap::Parser;
use console::style;
use std::fs;
use toasty::migration::{self, History, Snapshot};
use toasty::{
    Db,
    schema::{db::Schema, diff},
};

/// The only content of a new hand-written migration. The line is a comment, so
/// a file nobody edited applies as a no-op.
const PLACEHOLDER: &str = "-- Write this migration's SQL here.\n";

/// Creates a migration the models do not imply, to be written by hand.
///
/// Some SQL cannot be declared on a model: a trigger, a backfill, a virtual
/// table, a column default. This command writes the next numbered migration
/// file holding only a comment, a snapshot equal to the previous one, and the
/// history entry, exactly as `generate` lays down a generated migration. Write
/// the SQL in the file afterwards.
///
/// The command refuses when the models differ from the latest snapshot. Run
/// `generate` first, so that a change to the models never hides in a
/// hand-written step.
///
/// It asks no questions, so it runs without a terminal.
#[derive(Parser, Debug)]
pub struct NewCommand {
    /// Name for the migration
    name: String,
}

impl NewCommand {
    pub(crate) fn run(self, db: &Db, config: &Config) -> Result<()> {
        println!();
        println!(
            "  {}",
            style("New Hand-written Migration")
                .cyan()
                .bold()
                .underlined()
        );
        println!();

        let history_path = config.migration.get_history_file_path();
        let mut history = History::load_or_default(&history_path)?;

        let previous = history
            .entries()
            .last()
            .map(|entry| {
                Snapshot::load(
                    config
                        .migration
                        .get_snapshots_dir()
                        .join(&entry.snapshot_name),
                )
            })
            .transpose()?
            .unwrap_or_else(|| Snapshot::new(Schema::default()));

        let schema = Schema::clone(&db.schema().db);
        if migration::generate(
            db.driver(),
            &previous.schema,
            &schema,
            &diff::RenameHints::default(),
        )
        .is_some()
        {
            anyhow::bail!(
                "the models differ from the latest snapshot; run `migration generate` first, so that a model change is not hidden in a hand-written migration"
            );
        }

        fs::create_dir_all(config.migration.get_migrations_dir())?;
        fs::create_dir_all(config.migration.get_snapshots_dir())?;
        fs::create_dir_all(history_path.parent().unwrap())?;

        let migration_name = write_migration(
            config,
            &mut history,
            Some(&self.name),
            PLACEHOLDER,
            &previous
                .to_toml_string()
                .context("serializing the snapshot")?,
        )?;

        println!();
        println!(
            "  {}",
            style(format!(
                "Migration '{migration_name}' created; write its SQL in {}",
                config
                    .migration
                    .get_migrations_dir()
                    .join(&migration_name)
                    .display()
            ))
            .green()
            .bold()
        );
        println!();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MigrationConfig, migration::GenerateCommand};
    use tempfile::{TempDir, tempdir};
    use toasty::schema::db::Migration;

    #[derive(Debug, toasty::Model)]
    #[table = "articles"]
    struct Article {
        #[key]
        id: i64,
        title: String,
    }

    #[derive(Debug, toasty::Model)]
    #[table = "authors"]
    struct Author {
        #[key]
        id: i64,
        name: String,
    }

    async fn db(models: toasty::ModelSet) -> Db {
        Db::builder()
            .models(models)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    fn config(dir: &TempDir) -> Config {
        Config::new().migration(MigrationConfig::new().path(dir.path()))
    }

    fn generate(db: &Db, config: &Config, name: &str) {
        GenerateCommand::try_parse_from(["generate", "--name", name])
            .unwrap()
            .run(db, config)
            .unwrap();
    }

    fn new(db: &Db, config: &Config, name: &str) -> Result<()> {
        NewCommand::try_parse_from(["new", name])
            .unwrap()
            .run(db, config)
    }

    fn history(config: &Config) -> History {
        History::load_or_default(config.migration.get_history_file_path()).unwrap()
    }

    #[tokio::test]
    async fn a_new_migration_has_a_comment_for_its_sql_and_the_previous_snapshot() {
        let dir = tempdir().unwrap();
        let config = config(&dir);
        let db = db(toasty::models!(Article)).await;
        generate(&db, &config, "articles");

        new(&db, &config, "search_index").unwrap();

        let history = history(&config);
        let entries = history.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].name, "0001_search_index.sql");
        assert_eq!(entries[1].snapshot_name, "0001_snapshot.toml");
        assert_ne!(entries[0].id, entries[1].id);

        let sql =
            std::fs::read_to_string(config.migration.get_migrations_dir().join(&entries[1].name))
                .unwrap();
        assert_eq!(sql, PLACEHOLDER);
        // Applied as it is, the file sends the database nothing.
        assert!(Migration::Sql(sql).statements().is_empty());

        let snapshots = config.migration.get_snapshots_dir();
        assert_eq!(
            std::fs::read_to_string(snapshots.join("0001_snapshot.toml")).unwrap(),
            std::fs::read_to_string(snapshots.join("0000_snapshot.toml")).unwrap(),
        );
    }

    #[tokio::test]
    async fn a_new_migration_is_numbered_after_the_latest_one() {
        let dir = tempdir().unwrap();
        let config = config(&dir);
        let db = db(toasty::models!(Article)).await;
        generate(&db, &config, "articles");

        new(&db, &config, "first").unwrap();
        new(&db, &config, "second").unwrap();

        let names: Vec<_> = history(&config)
            .entries()
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        assert_eq!(
            names,
            ["0000_articles.sql", "0001_first.sql", "0002_second.sql"]
        );
    }

    #[tokio::test]
    async fn changed_models_are_refused_and_nothing_is_written() {
        let dir = tempdir().unwrap();
        let config = config(&dir);
        generate(&db(toasty::models!(Article)).await, &config, "articles");
        let changed = db(toasty::models!(Article, Author)).await;

        let err = new(&changed, &config, "search_index")
            .unwrap_err()
            .to_string();

        assert!(err.contains("migration generate"), "{err}");
        assert_eq!(history(&config).entries().len(), 1);
        assert_eq!(
            std::fs::read_dir(config.migration.get_migrations_dir())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_dir(config.migration.get_snapshots_dir())
                .unwrap()
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn models_that_were_never_generated_are_refused() {
        let dir = tempdir().unwrap();
        let config = config(&dir);

        let err = new(&db(toasty::models!(Article)).await, &config, "search_index")
            .unwrap_err()
            .to_string();

        assert!(err.contains("migration generate"), "{err}");
        assert!(history(&config).entries().is_empty());
    }
}
