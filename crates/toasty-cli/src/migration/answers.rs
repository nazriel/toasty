//! Answers to the questions `migration generate` asks about missing tables,
//! columns and indices, given as command-line flags instead of at a prompt.

use anyhow::{Result, bail};
use std::fmt;

/// What a question is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Table,
    Column,
    Index,
}

impl Kind {
    pub fn noun(self) -> &'static str {
        match self {
            Kind::Table => "table",
            Kind::Column => "column",
            Kind::Index => "index",
        }
    }

    fn rename_flag(self) -> &'static str {
        match self {
            Kind::Table => "--rename-table",
            Kind::Column => "--rename-column",
            Kind::Index => "--rename-index",
        }
    }

    fn drop_flag(self) -> &'static str {
        match self {
            Kind::Table => "--drop-table",
            Kind::Column => "--drop-column",
            Kind::Index => "--drop-index",
        }
    }

    /// Columns and indices are named together with their table.
    fn scoped(self) -> bool {
        !matches!(self, Kind::Table)
    }
}

/// A table, column or index that is in the previous snapshot but not in the
/// current schema, while the current schema has new ones of the same kind. It
/// was either dropped or renamed to one of the new ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Question {
    pub kind: Kind,
    /// The table a column or index belongs to, by its name in the previous
    /// snapshot. `None` for a table.
    pub scope: Option<String>,
    pub missing: String,
    /// The names it could have been renamed to.
    pub candidates: Vec<String>,
}

/// The answer to a [`Question`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Drop,
    /// Renamed to the candidate at this position.
    Rename(usize),
}

impl Question {
    /// The question as the prompt shows it.
    pub fn prompt(&self) -> String {
        match &self.scope {
            None => format!("  Table \"{}\" is missing", self.missing),
            Some(scope) => format!(
                "  {} \"{}\".\"{}\" is missing",
                capitalize(self.kind.noun()),
                scope,
                self.missing
            ),
        }
    }

    /// The flags that answer the question.
    fn flags(&self) -> String {
        let target = self.target();
        let renames: Vec<_> = self
            .candidates
            .iter()
            .map(|to| format!("{} {}={}", self.kind.rename_flag(), target, to))
            .collect();
        format!(
            "{}, or {} {}",
            renames.join(", or "),
            self.kind.drop_flag(),
            target
        )
    }

    /// `name`, or `table.name` for a column or an index.
    fn target(&self) -> String {
        match &self.scope {
            None => self.missing.clone(),
            Some(scope) => format!("{scope}.{}", self.missing),
        }
    }
}

impl fmt::Display for Question {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} \"{}\" is missing", self.kind.noun(), self.target())?;
        write!(f, " (new: ")?;
        for (i, candidate) in self.candidates.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "\"{candidate}\"")?;
        }
        write!(f, "); answer with {}", self.flags())
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// One `--rename-*` or `--drop-*` flag.
#[derive(Debug)]
struct Entry {
    kind: Kind,
    scope: Option<String>,
    name: String,
    /// The new name; `None` for a drop.
    to: Option<String>,
    /// The flag as it was written, for error messages.
    flag: String,
    used: bool,
}

/// The `--rename-*` and `--drop-*` flags of `migration generate`.
#[derive(Debug, Default)]
pub(crate) struct Answers {
    entries: Vec<Entry>,
}

impl Answers {
    /// Parses the flags' values. A rename is `old=new`; a drop is the name.
    /// A column or an index is named `table.name`, the table by its name in the
    /// previous snapshot.
    pub fn parse(
        rename_tables: &[String],
        rename_columns: &[String],
        rename_indices: &[String],
        drop_tables: &[String],
        drop_columns: &[String],
        drop_indices: &[String],
    ) -> Result<Self> {
        let mut answers = Self::default();
        for (kind, values) in [
            (Kind::Table, rename_tables),
            (Kind::Column, rename_columns),
            (Kind::Index, rename_indices),
        ] {
            for value in values {
                let flag = format!("{} {value}", kind.rename_flag());
                let Some((from, to)) = value.split_once('=') else {
                    bail!("{flag}: expected {}", expected(kind, true));
                };
                if to.is_empty() {
                    bail!("{flag}: expected {}", expected(kind, true));
                }
                answers.push(kind, from, Some(to), flag)?;
            }
        }
        for (kind, values) in [
            (Kind::Table, drop_tables),
            (Kind::Column, drop_columns),
            (Kind::Index, drop_indices),
        ] {
            for value in values {
                let flag = format!("{} {value}", kind.drop_flag());
                answers.push(kind, value, None, flag)?;
            }
        }
        Ok(answers)
    }

    fn push(&mut self, kind: Kind, target: &str, to: Option<&str>, flag: String) -> Result<()> {
        let (scope, name) = if kind.scoped() {
            match target.split_once('.') {
                Some((scope, name)) if !scope.is_empty() && !name.is_empty() => {
                    (Some(scope.to_string()), name)
                }
                _ => bail!("{flag}: expected {}", expected(kind, to.is_some())),
            }
        } else if target.is_empty() {
            bail!("{flag}: expected {}", expected(kind, to.is_some()));
        } else {
            (None, target)
        };

        self.entries.push(Entry {
            kind,
            scope,
            name: name.to_string(),
            to: to.map(str::to_string),
            flag,
            used: false,
        });
        Ok(())
    }

    /// Looks for the flag that answers `question`. `None` if there is none.
    pub fn answer(&mut self, question: &Question) -> Result<Option<Answer>> {
        let matching: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                e.kind == question.kind && e.scope == question.scope && e.name == question.missing
            })
            .map(|(i, _)| i)
            .collect();

        let [index] = matching[..] else {
            if matching.is_empty() {
                return Ok(None);
            }
            let flags: Vec<_> = matching
                .iter()
                .map(|&i| self.entries[i].flag.as_str())
                .collect();
            bail!(
                "conflicting flags for one {}: {}",
                question.kind.noun(),
                flags.join(", ")
            );
        };

        let entry = &mut self.entries[index];
        entry.used = true;
        let Some(to) = &entry.to else {
            return Ok(Some(Answer::Drop));
        };
        match question.candidates.iter().position(|c| c == to) {
            Some(position) => Ok(Some(Answer::Rename(position))),
            None => bail!(
                "{}: no new {} is named \"{to}\"; the new {}s are {}",
                entry.flag,
                question.kind.noun(),
                question.kind.noun(),
                quoted(&question.candidates),
            ),
        }
    }

    /// Records that the diff drops this item, whether or not it is a
    /// question. A drop flag for it is then not left over.
    pub fn note_dropped(&mut self, kind: Kind, scope: Option<&str>, name: &str) {
        for entry in &mut self.entries {
            if entry.to.is_none()
                && entry.kind == kind
                && entry.scope.as_deref() == scope
                && entry.name == name
            {
                entry.used = true;
            }
        }
    }

    /// Fails on a flag that named nothing in the diff.
    pub fn finish(&self) -> Result<()> {
        let unused: Vec<_> = self
            .entries
            .iter()
            .filter(|e| !e.used)
            .map(|e| {
                format!(
                    "  {}: no such missing {} in the schema diff",
                    e.flag,
                    e.kind.noun()
                )
            })
            .collect();
        if unused.is_empty() {
            return Ok(());
        }
        bail!(
            "flags that name nothing in the schema diff:\n{}",
            unused.join("\n")
        );
    }
}

fn expected(kind: Kind, rename: bool) -> String {
    let name = if kind.scoped() { "table.name" } else { "name" };
    if rename {
        format!("`{name}=new_name`")
    } else {
        format!("`{name}`")
    }
}

fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Builds the error for questions that nothing answered.
pub(crate) fn unanswered(questions: &[Question]) -> anyhow::Error {
    let lines: Vec<_> = questions.iter().map(|q| format!("  {q}")).collect();
    anyhow::anyhow!(
        "cannot ask which of these were renamed and which dropped without a terminal; \
         answer them with flags:\n{}",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question(kind: Kind, scope: Option<&str>, missing: &str, candidates: &[&str]) -> Question {
        Question {
            kind,
            scope: scope.map(str::to_string),
            missing: missing.to_string(),
            candidates: candidates.iter().map(|c| c.to_string()).collect(),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn a_rename_flag_picks_the_candidate_by_name() {
        let mut answers =
            Answers::parse(&strings(&["users=accounts"]), &[], &[], &[], &[], &[]).unwrap();
        let q = question(Kind::Table, None, "users", &["orders", "accounts"]);
        assert_eq!(answers.answer(&q).unwrap(), Some(Answer::Rename(1)));
        answers.finish().unwrap();
    }

    #[test]
    fn a_column_is_named_with_its_table() {
        let mut answers =
            Answers::parse(&[], &strings(&["users.a=b"]), &[], &[], &[], &[]).unwrap();
        let elsewhere = question(Kind::Column, Some("orders"), "a", &["b"]);
        assert_eq!(answers.answer(&elsewhere).unwrap(), None);
        let q = question(Kind::Column, Some("users"), "a", &["b"]);
        assert_eq!(answers.answer(&q).unwrap(), Some(Answer::Rename(0)));
        answers.finish().unwrap();
    }

    #[test]
    fn a_drop_flag_answers_drop() {
        let mut answers =
            Answers::parse(&[], &[], &[], &[], &[], &strings(&["users.by_email"])).unwrap();
        let q = question(Kind::Index, Some("users"), "by_email", &["by_name"]);
        assert_eq!(answers.answer(&q).unwrap(), Some(Answer::Drop));
        answers.finish().unwrap();
    }

    #[test]
    fn no_flag_is_no_answer() {
        let mut answers = Answers::default();
        let q = question(Kind::Table, None, "users", &["accounts"]);
        assert_eq!(answers.answer(&q).unwrap(), None);
        answers.finish().unwrap();
    }

    #[test]
    fn a_rename_to_an_unknown_name_is_an_error() {
        let mut answers =
            Answers::parse(&strings(&["users=nobody"]), &[], &[], &[], &[], &[]).unwrap();
        let q = question(Kind::Table, None, "users", &["accounts"]);
        let err = answers.answer(&q).unwrap_err().to_string();
        assert!(err.contains("--rename-table users=nobody"), "{err}");
        assert!(err.contains("\"accounts\""), "{err}");
    }

    #[test]
    fn a_flag_that_names_nothing_in_the_diff_is_an_error() {
        let mut answers = Answers::parse(
            &strings(&["ghost=accounts"]),
            &[],
            &[],
            &strings(&["phantom"]),
            &[],
            &[],
        )
        .unwrap();
        let q = question(Kind::Table, None, "users", &["accounts"]);
        assert_eq!(answers.answer(&q).unwrap(), None);
        let err = answers.finish().unwrap_err().to_string();
        assert!(err.contains("--rename-table ghost=accounts"), "{err}");
        assert!(err.contains("--drop-table phantom"), "{err}");
    }

    #[test]
    fn a_drop_flag_for_an_unambiguous_drop_is_not_left_over() {
        let mut answers = Answers::parse(&[], &[], &[], &strings(&["users"]), &[], &[]).unwrap();
        answers.note_dropped(Kind::Table, None, "users");
        answers.finish().unwrap();
    }

    #[test]
    fn two_flags_for_one_item_conflict() {
        let mut answers = Answers::parse(
            &strings(&["users=accounts"]),
            &[],
            &[],
            &strings(&["users"]),
            &[],
            &[],
        )
        .unwrap();
        let q = question(Kind::Table, None, "users", &["accounts"]);
        let err = answers.answer(&q).unwrap_err().to_string();
        assert!(err.contains("conflicting"), "{err}");
    }

    #[test]
    fn malformed_flags_are_errors() {
        assert!(Answers::parse(&strings(&["users"]), &[], &[], &[], &[], &[]).is_err());
        assert!(Answers::parse(&strings(&["users="]), &[], &[], &[], &[], &[]).is_err());
        assert!(Answers::parse(&[], &strings(&["a=b"]), &[], &[], &[], &[]).is_err());
        assert!(Answers::parse(&[], &[], &[], &[], &strings(&["users"]), &[]).is_err());
    }

    #[test]
    fn the_error_for_unanswered_questions_lists_every_flag() {
        let questions = [
            question(Kind::Table, None, "users", &["accounts"]),
            question(Kind::Column, Some("orders"), "a", &["b", "c"]),
        ];
        let err = unanswered(&questions).to_string();
        assert!(
            err.contains("--rename-table users=accounts, or --drop-table users"),
            "{err}"
        );
        assert!(
            err.contains(
                "--rename-column orders.a=b, or --rename-column orders.a=c, or --drop-column orders.a"
            ),
            "{err}"
        );
    }
}
