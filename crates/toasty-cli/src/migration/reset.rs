use super::apply::apply_migrations;
use crate::Config;
use crate::theme::dialoguer_theme;
use crate::utility::is_interactive;
use anyhow::Result;
use clap::Parser;
use console::style;
use dialoguer::Confirm;
use toasty::Db;

/// Drops all tables in the database, then optionally re-applies migrations.
///
/// Prompts for confirmation before proceeding. After the reset, all
/// migrations from the history file are re-applied unless `--skip-migrations`
/// is passed.
#[derive(Parser, Debug)]
pub struct ResetCommand {
    /// Skip applying migrations after reset
    #[arg(long)]
    skip_migrations: bool,
}

impl ResetCommand {
    pub(crate) async fn run(self, db: &Db, config: &Config) -> Result<()> {
        // The confirmation cannot be answered without a terminal, and the
        // reset must not go ahead without it.
        if !is_interactive() {
            anyhow::bail!("cannot ask for confirmation to reset the database without a terminal");
        }

        println!();
        println!("  {}", style("Reset Database").cyan().bold().underlined());
        println!();
        println!(
            "  {}",
            style(format!(
                "Connected to {}",
                crate::utility::redact_url_password(&db.driver().url())
            ))
            .dim()
        );
        println!();

        let theme = {
            let mut t = dialoguer_theme();
            t.success_prefix = style(" ".to_string());
            t.prompt_prefix = style(" ".to_string());
            t.prompt_style = console::Style::new().red().bold();
            t
        };

        let confirmed = Confirm::with_theme(&theme)
            .with_prompt("This will drop all tables and data. Are you sure?")
            .default(false)
            .interact()?;

        if !confirmed {
            println!();
            println!("  {}", style("Aborted.").dim());
            println!();
            return Ok(());
        }

        println!();
        println!("  {} Resetting database...", style("→").cyan());

        db.reset_db().await?;

        println!(
            "  {} {}",
            style("✓").green().bold(),
            style("Database reset successfully").dim()
        );
        println!();

        if !self.skip_migrations {
            apply_migrations(db, config).await?;
        }

        Ok(())
    }
}
