use super::answers::{self, Answer, Answers, Kind, Question};
use crate::{Config, theme::dialoguer_theme, utility::is_interactive};
use anyhow::{Context, Result};
use clap::Parser;
use console::style;
use dialoguer::{Confirm, Select};
use hashbrown::{HashMap, HashSet};
use rand::RngExt;
use std::fs;
use toasty::migration::{self, History, HistoryEntry, Snapshot};
use toasty::{
    Db,
    schema::{
        db::{ColumnId, IndexId, Migration, Schema, TableId},
        diff,
    },
};

/// Generates a new SQL migration from the current schema diff.
///
/// Compares the current database schema (as registered on the [`Db`]) against
/// the most recent snapshot. If there are differences, generates a SQL
/// migration file, writes a new snapshot, and updates the history file.
///
/// When the diff contains dropped-and-added tables, columns, or indices, the
/// command asks whether these are renames rather than drop-then-create pairs.
/// Choosing to drop must be confirmed, and the confirmation defaults to no.
///
/// The questions can be answered with the `--rename-*` and `--drop-*` flags
/// instead, which lets the command run without a terminal. Without a terminal,
/// a question that no flag answers is an error: the command never assumes
/// that something was dropped.
///
/// If no schema changes are detected, the command exits without creating any
/// files.
#[derive(Parser, Debug)]
pub struct GenerateCommand {
    /// Name for the migration
    #[arg(short, long)]
    name: Option<String>,

    /// Answer that a table was renamed: `old=new`. Can be repeated.
    #[arg(long, value_name = "OLD=NEW")]
    rename_table: Vec<String>,

    /// Answer that a column was renamed: `table.old=new`, with the table's
    /// name in the previous snapshot. Can be repeated.
    #[arg(long, value_name = "TABLE.OLD=NEW")]
    rename_column: Vec<String>,

    /// Answer that an index was renamed: `table.old=new`, with the table's
    /// name in the previous snapshot. Can be repeated.
    #[arg(long, value_name = "TABLE.OLD=NEW")]
    rename_index: Vec<String>,

    /// Answer that a table was dropped: `name`. Can be repeated.
    #[arg(long, value_name = "NAME")]
    drop_table: Vec<String>,

    /// Answer that a column was dropped: `table.name`. Can be repeated.
    #[arg(long, value_name = "TABLE.NAME")]
    drop_column: Vec<String>,

    /// Answer that an index was dropped: `table.name`. Can be repeated.
    #[arg(long, value_name = "TABLE.NAME")]
    drop_index: Vec<String>,
}

/// Asks one question at the terminal.
type Ask<'a> = &'a mut dyn FnMut(&Question) -> Result<Answer>;

/// Asks the question on the terminal. The prompt opens on the first rename, not
/// on dropping, and dropping needs a confirmation that defaults to no.
fn ask_on_terminal(question: &Question) -> Result<Answer> {
    let noun = question.kind.noun();
    let mut options = vec![format!("  Drop \"{}\" ✖", question.missing)];
    for candidate in &question.candidates {
        options.push(format!(
            "  Rename \"{}\" → \"{}\"",
            question.missing, candidate
        ));
    }

    loop {
        let selection = Select::with_theme(&dialoguer_theme())
            .with_prompt(question.prompt())
            .items(&options)
            .default(1)
            .interact()?;

        if selection > 0 {
            return Ok(Answer::Rename(selection - 1));
        }

        let confirmed = Confirm::with_theme(&dialoguer_theme())
            .with_prompt(format!(
                "  Drop {noun} \"{}\"? Its data is lost",
                question.missing
            ))
            .default(false)
            .interact()?;
        if confirmed {
            return Ok(Answer::Drop);
        }
    }
}

/// Answers a question from the flags, or else at the terminal. Without a
/// terminal, the question is recorded in `unanswered` and answered with a
/// placeholder; the caller must fail before it generates anything.
fn resolve(
    question: Question,
    answers: &mut Answers,
    ask: &mut Option<Ask<'_>>,
    unanswered: &mut Vec<Question>,
) -> Result<Answer> {
    if let Some(answer) = answers.answer(&question)? {
        return Ok(answer);
    }

    match ask {
        Some(ask) => ask(&question).with_context(|| format!("no answer was read for: {question}")),
        None => {
            unanswered.push(question);
            Ok(Answer::Drop)
        }
    }
}

/// Collects rename hints by asking about potential renames: from the flags in
/// `answers`, then at the terminal through `ask`. With no `ask`, a question the
/// flags do not answer is an error.
fn collect_rename_hints(
    previous_schema: &Schema,
    schema: &Schema,
    answers: &mut Answers,
    mut ask: Option<Ask<'_>>,
) -> Result<diff::RenameHints> {
    let mut unanswered = Vec::new();
    let mut hints = diff::RenameHints::default();
    let mut ignored_tables = HashSet::<TableId>::new();
    let mut ignored_columns = HashMap::<TableId, HashSet<ColumnId>>::new();
    let mut ignored_indices = HashMap::<TableId, HashSet<IndexId>>::new();

    'main: loop {
        let diff = diff::Schema::from(previous_schema, schema, &hints);

        // Check for table renames
        let dropped_tables: Vec<_> = diff
            .tables()
            .iter()
            .filter_map(|item| match item {
                diff::Table::Drop(table) if !ignored_tables.contains(&table.id) => Some(*table),
                _ => None,
            })
            .collect();

        let added_tables: Vec<_> = diff
            .tables()
            .iter()
            .filter_map(|item| match item {
                diff::Table::Create(table) => Some(*table),
                _ => None,
            })
            .collect();

        for dropped_table in &dropped_tables {
            answers.note_dropped(Kind::Table, None, &dropped_table.name);
        }

        // If there are both dropped and added tables, ask about potential renames
        if !dropped_tables.is_empty() && !added_tables.is_empty() {
            for dropped_table in &dropped_tables {
                let question = Question {
                    kind: Kind::Table,
                    scope: None,
                    missing: dropped_table.name.clone(),
                    candidates: added_tables.iter().map(|t| t.name.clone()).collect(),
                };

                match resolve(question, answers, &mut ask, &mut unanswered)? {
                    Answer::Drop => {
                        // It was dropped
                        ignored_tables.insert(dropped_table.id);
                    }
                    Answer::Rename(position) => {
                        let to_table = added_tables[position];
                        drop(diff);
                        hints.add_table_hint(dropped_table.id, to_table.id);
                        continue 'main; // Regenerate diff with new hint
                    }
                }
            }
        }

        // Check for column and index renames within altered tables
        for item in diff.tables().iter() {
            if let diff::Table::Alter {
                previous,
                next: _,
                columns,
                indices,
            } = item
            {
                // Handle column renames
                let dropped_columns: Vec<_> = columns
                    .iter()
                    .filter_map(|item| match item {
                        diff::Column::Drop(column)
                            if !ignored_columns
                                .get(&previous.id)
                                .is_some_and(|set| set.contains(&column.id)) =>
                        {
                            Some(*column)
                        }
                        _ => None,
                    })
                    .collect();

                let added_columns: Vec<_> = columns
                    .iter()
                    .filter_map(|item| match item {
                        diff::Column::Add(column) => Some(*column),
                        _ => None,
                    })
                    .collect();

                for dropped_column in &dropped_columns {
                    answers.note_dropped(Kind::Column, Some(&previous.name), &dropped_column.name);
                }

                if !dropped_columns.is_empty() && !added_columns.is_empty() {
                    for dropped_column in &dropped_columns {
                        let question = Question {
                            kind: Kind::Column,
                            scope: Some(previous.name.clone()),
                            missing: dropped_column.name.clone(),
                            candidates: added_columns.iter().map(|c| c.name.clone()).collect(),
                        };

                        match resolve(question, answers, &mut ask, &mut unanswered)? {
                            Answer::Drop => {
                                // It was dropped
                                ignored_columns
                                    .entry(previous.id)
                                    .or_default()
                                    .insert(dropped_column.id);
                            }
                            Answer::Rename(position) => {
                                let next_column = added_columns[position];
                                drop(diff);
                                hints.add_column_hint(dropped_column.id, next_column.id);
                                continue 'main; // Regenerate diff with new hint
                            }
                        }
                    }
                }

                // Handle index renames
                let dropped_indices: Vec<_> = indices
                    .iter()
                    .filter_map(|item| match item {
                        diff::Index::Drop(index)
                            if !ignored_indices
                                .get(&previous.id)
                                .is_some_and(|set| set.contains(&index.id)) =>
                        {
                            Some(*index)
                        }
                        _ => None,
                    })
                    .collect();

                let added_indices: Vec<_> = indices
                    .iter()
                    .filter_map(|item| match item {
                        diff::Index::Create(index) => Some(*index),
                        _ => None,
                    })
                    .collect();

                for dropped_index in &dropped_indices {
                    answers.note_dropped(Kind::Index, Some(&previous.name), &dropped_index.name);
                }

                if !dropped_indices.is_empty() && !added_indices.is_empty() {
                    for dropped_index in &dropped_indices {
                        let question = Question {
                            kind: Kind::Index,
                            scope: Some(previous.name.clone()),
                            missing: dropped_index.name.clone(),
                            candidates: added_indices.iter().map(|i| i.name.clone()).collect(),
                        };

                        match resolve(question, answers, &mut ask, &mut unanswered)? {
                            Answer::Drop => {
                                // It was dropped
                                ignored_indices
                                    .entry(previous.id)
                                    .or_default()
                                    .insert(dropped_index.id);
                            }
                            Answer::Rename(position) => {
                                let to_index = added_indices[position];
                                drop(diff);
                                hints.add_index_hint(dropped_index.id, to_index.id);
                                continue 'main; // Regenerate diff with new hint
                            }
                        }
                    }
                }
            }
        }

        // No more potential renames to ask about
        break;
    }

    // A flag that matched nothing is reported with the questions it was meant
    // for: a misspelled name leaves both behind.
    let leftover = answers.finish().err();
    if !unanswered.is_empty() {
        let mut message = answers::unanswered(&unanswered).to_string();
        if let Some(leftover) = leftover {
            message.push('\n');
            message.push_str(&leftover.to_string());
        }
        anyhow::bail!(message);
    }
    if let Some(leftover) = leftover {
        return Err(leftover);
    }

    Ok(hints)
}

/// Writes a migration's SQL file and its snapshot, adds the history entry and
/// saves the history, numbering all three as the config says. Returns the
/// migration file's name.
///
/// This is the one place that lays a migration down on disk, shared by
/// `generate` and `new`, so a hand-written step is indistinguishable from a
/// generated one.
pub(super) fn write_migration(
    config: &Config,
    history: &mut History,
    name: Option<&str>,
    sql: &str,
    snapshot: &str,
) -> Result<String> {
    let migration_prefix = match config.migration.prefix_style {
        crate::MigrationPrefixStyle::Sequential => {
            format!("{:04}", history.next_migration_number())
        }
        crate::MigrationPrefixStyle::Timestamp => {
            jiff::Timestamp::now().strftime("%Y%m%d_%H%M%S").to_string()
        }
    };
    let snapshot_name = format!("{migration_prefix:04}_snapshot.toml");
    let snapshot_path = config.migration.get_snapshots_dir().join(&snapshot_name);

    let migration_name = format!(
        "{:04}_{}.sql",
        migration_prefix,
        name.unwrap_or("migration")
    );
    let migration_path = config.migration.get_migrations_dir().join(&migration_name);

    history.add_entry(HistoryEntry {
        // Some databases only supported signed 64-bit integers.
        id: rand::rng().random_range(0..i64::MAX) as u64,
        name: migration_name.clone(),
        snapshot_name: snapshot_name.clone(),
        checksum: None,
    });

    std::fs::write(&migration_path, sql)?;
    println!(
        "  {} {}",
        style("✓").green().bold(),
        style(format!("Created migration file: {}", migration_name)).dim()
    );

    std::fs::write(&snapshot_path, snapshot)?;
    println!(
        "  {} {}",
        style("✓").green().bold(),
        style(format!("Created snapshot: {}", snapshot_name)).dim()
    );

    history.save(config.migration.get_history_file_path())?;
    println!(
        "  {} {}",
        style("✓").green().bold(),
        style("Updated migration history").dim()
    );

    Ok(migration_name)
}

impl GenerateCommand {
    pub(crate) fn run(self, db: &Db, config: &Config) -> Result<()> {
        println!();
        println!(
            "  {}",
            style("Generate Migration").cyan().bold().underlined()
        );
        println!();

        let history_path = config.migration.get_history_file_path();

        fs::create_dir_all(config.migration.get_migrations_dir())?;
        fs::create_dir_all(config.migration.get_snapshots_dir())?;
        fs::create_dir_all(history_path.parent().unwrap())?;

        let mut history = History::load_or_default(&history_path)?;

        let previous_snapshot = history
            .entries()
            .last()
            .map(|f| Snapshot::load(config.migration.get_snapshots_dir().join(&f.snapshot_name)))
            .transpose()?;
        let previous_schema = previous_snapshot
            .map(|snapshot| snapshot.schema)
            .unwrap_or_else(Schema::default);

        let schema = toasty::schema::db::Schema::clone(&db.schema().db);

        let mut answers = Answers::parse(
            &self.rename_table,
            &self.rename_column,
            &self.rename_index,
            &self.drop_table,
            &self.drop_column,
            &self.drop_index,
        )?;
        let ask: Option<Ask<'_>> = if is_interactive() {
            Some(&mut ask_on_terminal)
        } else {
            None
        };
        let rename_hints = collect_rename_hints(&previous_schema, &schema, &mut answers, ask)?;
        let Some(generated) =
            migration::generate(db.driver(), &previous_schema, &schema, &rename_hints)
        else {
            println!(
                "  {}",
                style("The current schema matches the previous snapshot. No migration needed.")
                    .magenta()
                    .dim()
            );
            println!();
            return Ok(());
        };

        let Migration::Sql(sql) = generated.migration;
        let migration_name = write_migration(
            config,
            &mut history,
            self.name.as_deref(),
            &format!("{sql}\n"),
            &generated.snapshot.to_toml_string()?,
        )?;

        println!();
        println!(
            "  {}",
            style(format!(
                "Migration '{}' generated successfully",
                migration_name
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
    use crate::migration::fixtures::{describe, schema, table};

    fn flags(
        rename_tables: &[&str],
        rename_columns: &[&str],
        rename_indices: &[&str],
        drop_tables: &[&str],
        drop_columns: &[&str],
        drop_indices: &[&str],
    ) -> Answers {
        let owned = |values: &[&str]| values.iter().map(|v| v.to_string()).collect::<Vec<_>>();
        Answers::parse(
            &owned(rename_tables),
            &owned(rename_columns),
            &owned(rename_indices),
            &owned(drop_tables),
            &owned(drop_columns),
            &owned(drop_indices),
        )
        .unwrap()
    }

    fn column_rename() -> (Schema, Schema) {
        (
            schema(vec![table(0, "users", &["email", "name"], &[])]),
            schema(vec![table(0, "users", &["mail", "name"], &[])]),
        )
    }

    #[test]
    fn a_rename_flag_renames_a_column_without_a_terminal() {
        let (previous, next) = column_rename();
        let mut answers = flags(&[], &["users.email=mail"], &[], &[], &[], &[]);

        let hints = collect_rename_hints(&previous, &next, &mut answers, None).unwrap();

        assert_eq!(
            describe(&previous, &next, &hints),
            ["rename column users.email to mail"]
        );
    }

    #[test]
    fn a_drop_flag_drops_a_column_without_a_terminal() {
        let (previous, next) = column_rename();
        let mut answers = flags(&[], &[], &[], &[], &["users.email"], &[]);

        let hints = collect_rename_hints(&previous, &next, &mut answers, None).unwrap();

        assert_eq!(
            describe(&previous, &next, &hints),
            ["drop column users.email", "add column users.mail"]
        );
    }

    #[test]
    fn a_rename_flag_renames_a_table_without_a_terminal() {
        let previous = schema(vec![table(0, "users", &["email"], &[])]);
        let next = schema(vec![table(0, "accounts", &["email"], &[])]);
        // The primary key's index is named after the table, so it is a question too.
        let mut answers = flags(
            &["users=accounts"],
            &[],
            &["users.index_users_by_id=index_accounts_by_id"],
            &[],
            &[],
            &[],
        );

        let hints = collect_rename_hints(&previous, &next, &mut answers, None).unwrap();

        assert_eq!(
            describe(&previous, &next, &hints),
            [
                "rename table users to accounts",
                "rename index users.index_users_by_id to index_accounts_by_id"
            ]
        );
    }

    #[test]
    fn a_rename_flag_renames_an_index_without_a_terminal() {
        let previous = schema(vec![table(0, "users", &["email"], &["by_email"])]);
        let next = schema(vec![table(0, "users", &["email"], &["by_mail"])]);
        let mut answers = flags(&[], &[], &["users.by_email=by_mail"], &[], &[], &[]);

        let hints = collect_rename_hints(&previous, &next, &mut answers, None).unwrap();

        assert_eq!(
            describe(&previous, &next, &hints),
            ["rename index users.by_email to by_mail"]
        );
    }

    #[test]
    fn a_missing_answer_without_a_terminal_is_an_error_that_names_the_flags() {
        let previous = schema(vec![
            table(0, "users", &["email", "name"], &[]),
            table(1, "orders", &["total"], &[]),
        ]);
        let next = schema(vec![
            table(0, "users", &["mail", "name"], &[]),
            table(1, "orders", &["sum"], &[]),
        ]);
        let mut answers = flags(&[], &["users.email=mail"], &[], &[], &[], &[]);

        let err = collect_rename_hints(&previous, &next, &mut answers, None)
            .err()
            .unwrap()
            .to_string();

        assert!(err.contains("without a terminal"), "{err}");
        assert!(!err.contains("users.email"), "{err}");
        assert!(
            err.contains("--rename-column orders.total=sum, or --drop-column orders.total"),
            "{err}"
        );
    }

    #[test]
    fn a_flag_that_names_nothing_without_a_terminal_is_an_error() {
        let (previous, next) = column_rename();
        let mut answers = flags(
            &[],
            &["users.email=mail", "users.ghost=mail"],
            &[],
            &[],
            &[],
            &[],
        );

        let err = collect_rename_hints(&previous, &next, &mut answers, None)
            .err()
            .unwrap()
            .to_string();

        assert!(err.contains("--rename-column users.ghost=mail"), "{err}");
        assert!(!err.contains("--rename-column users.email=mail"), "{err}");
    }

    #[test]
    fn a_misspelled_flag_is_reported_with_the_question_it_was_meant_for() {
        let (previous, next) = column_rename();
        let mut answers = flags(&[], &["users.emial=mail"], &[], &[], &[], &[]);

        let err = collect_rename_hints(&previous, &next, &mut answers, None)
            .err()
            .unwrap()
            .to_string();

        assert!(err.contains("--rename-column users.email=mail"), "{err}");
        assert!(err.contains("--rename-column users.emial=mail"), "{err}");
    }

    #[test]
    fn a_rename_flag_for_a_name_that_is_not_new_is_an_error() {
        let (previous, next) = column_rename();
        let mut answers = flags(&[], &["users.email=nope"], &[], &[], &[], &[]);

        let err = collect_rename_hints(&previous, &next, &mut answers, None)
            .err()
            .unwrap()
            .to_string();

        assert!(err.contains("--rename-column users.email=nope"), "{err}");
    }

    #[test]
    fn the_terminal_is_asked_only_what_the_flags_leave_open() {
        let previous = schema(vec![table(0, "users", &["email", "name"], &[])]);
        let next = schema(vec![table(0, "users", &["mail", "label"], &[])]);
        let mut answers = flags(&[], &["users.email=mail"], &[], &[], &[], &[]);
        let mut asked = Vec::new();
        let mut ask = |question: &Question| {
            asked.push(question.missing.clone());
            Ok(Answer::Rename(0))
        };

        let hints = collect_rename_hints(&previous, &next, &mut answers, Some(&mut ask)).unwrap();

        assert_eq!(asked, ["name"]);
        assert_eq!(
            describe(&previous, &next, &hints),
            [
                "rename column users.email to mail",
                "rename column users.name to label"
            ]
        );
    }

    #[test]
    fn a_failed_prompt_is_an_error_that_names_the_flags() {
        let (previous, next) = column_rename();
        let mut answers = Answers::default();
        let mut ask = |_: &Question| Err(anyhow::anyhow!("unexpected end of input"));

        let err = format!(
            "{:#}",
            collect_rename_hints(&previous, &next, &mut answers, Some(&mut ask))
                .err()
                .unwrap()
        );

        assert!(err.contains("--drop-column users.email"), "{err}");
        assert!(err.contains("unexpected end of input"), "{err}");
    }

    #[test]
    fn no_questions_need_no_answers_and_no_terminal() {
        let both = schema(vec![table(0, "users", &["email"], &["by_email"])]);
        let mut answers = Answers::default();

        let hints = collect_rename_hints(&both, &both, &mut answers, None).unwrap();

        assert!(describe(&both, &both, &hints).is_empty());
    }
}
