use crate::{Config, utility::is_stdout_terminal};
use anyhow::Result;
use clap::Parser;
use console::style;
use toasty::Db;
use toasty::migration::Snapshot;

/// Prints the current schema as a TOML snapshot to stdout.
///
/// Reads the schema registered on the [`Db`] and formats it as a
/// [`Snapshot`]. On a terminal, table headers, key-value pairs, and whitespace
/// are syntax-highlighted. When stdout is not a terminal, or with `--raw`, the
/// output is the snapshot file's exact content, which can be redirected into a
/// file.
#[derive(Parser, Debug)]
pub struct SnapshotCommand {
    /// Print the snapshot as it is stored in a snapshot file, without a
    /// heading, indentation or colors
    #[arg(long)]
    raw: bool,
}

impl SnapshotCommand {
    /// Raw output is for a pipe or a file, or when `--raw` asks for it.
    fn prints_raw(&self, stdout_is_terminal: bool) -> bool {
        self.raw || !stdout_is_terminal
    }

    pub(crate) fn run(self, db: &Db, _config: &Config) -> Result<()> {
        let snapshot = Snapshot::new(toasty::schema::db::Schema::clone(&db.schema().db));

        if self.prints_raw(is_stdout_terminal()) {
            print!("{}", snapshot.to_toml_string()?);
            return Ok(());
        }

        println!();
        println!(
            "  {}",
            style("Current Schema Snapshot").cyan().bold().underlined()
        );
        println!();

        // Print the snapshot with nice formatting
        let snapshot_str = snapshot.to_toml_string()?;
        for line in snapshot_str.lines() {
            if line.starts_with('[') {
                println!("  {}", style(line).yellow().bold());
            } else if line.contains('=') {
                let parts: Vec<&str> = line.splitn(2, '=').collect();
                if parts.len() == 2 {
                    println!(
                        "  {}{} {}",
                        style(parts[0]).cyan(),
                        style("=").dim(),
                        style(parts[1]).green()
                    );
                } else {
                    println!("  {}", style(line).dim());
                }
            } else if line.trim().is_empty() {
                println!();
            } else {
                println!("  {}", style(line).dim());
            }
        }

        println!();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::fixtures::{schema, table};
    use tempfile::tempdir;

    #[test]
    fn raw_is_printed_unless_stdout_is_a_terminal() {
        let decorated = SnapshotCommand { raw: false };
        assert!(decorated.prints_raw(false));
        assert!(!decorated.prints_raw(true));

        let raw = SnapshotCommand { raw: true };
        assert!(raw.prints_raw(true));
        assert!(raw.prints_raw(false));
    }

    #[test]
    fn raw_output_written_to_a_file_loads_as_the_same_snapshot() {
        let snapshot = Snapshot::new(schema(vec![
            table(0, "users", &["email", "name"], &["by_email"]),
            table(1, "orders", &["total"], &[]),
        ]));

        // What the command prints in raw mode is `to_toml_string()`, byte for byte.
        let printed = snapshot.to_toml_string().unwrap();
        let dir = tempdir().unwrap();
        let path = dir.path().join("0001_snapshot.toml");
        std::fs::write(&path, &printed).unwrap();

        let loaded = Snapshot::load(&path).unwrap();
        assert_eq!(loaded.to_toml_string().unwrap(), printed);
        assert_eq!(loaded.schema.tables.len(), 2);
        assert_eq!(loaded.schema.tables[0].name, "users");
    }
}
