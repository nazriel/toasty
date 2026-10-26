use std::collections::HashSet;

use toasty_core::{
    driver::Dialect,
    stmt::{self, Visit},
};

use crate::engine::{exec::ConditionalOutput, exec::Exec, mir};

/// SQLite's default `SQLITE_MAX_VARIABLE_NUMBER` since 3.32.
const SQLITE_MAX_VARIABLES: usize = 32_766;

impl Exec<'_> {
    /// Splits a batch load whose bound keys would exceed SQLite's variable
    /// limit into chunks that fit.
    ///
    /// A batch load binds every key of its input as a variable. This returns
    /// `None`, and the statement runs as is, unless the statement is a plain
    /// SQLite query over one batch input that would bind more variables than
    /// the limit allows. Otherwise it returns the position of the batch input
    /// and the chunks of its rows, in order; the caller runs the statement once
    /// per chunk, substituting the chunk for the input, and concatenates the
    /// rows.
    ///
    /// Concatenating is only equivalent to an unsplit load when the statement
    /// neither orders, limits, groups nor removes duplicates across the batch,
    /// so any such statement is left alone. A row that matches batch rows from
    /// two chunks would come back twice, so identical batch rows are collapsed
    /// before the input is split.
    pub(super) fn split_batch_load(
        &self,
        action: &mir::ExecStatement,
        input_values: &[stmt::Value],
    ) -> Option<(usize, Vec<Vec<stmt::Value>>)> {
        if self.engine.capability().sql != Some(Dialect::Sqlite)
            || action.conditional != ConditionalOutput::None
            || action.pagination.is_some()
        {
            return None;
        }

        let stmt::Statement::Query(query) = &action.stmt else {
            return None;
        };

        let stmt::ExprSet::Select(select) = &query.body else {
            return None;
        };

        if query.with.is_some()
            || query.order_by.is_some()
            || query.limit.is_some()
            || !query.locks.is_empty()
            || select.distinct
            || !select.group_by.is_empty()
            || select.having.is_some()
        {
            return None;
        }

        let mut scan = Scan::default();
        scan.visit(&action.stmt);

        let [position] = scan.batch_positions[..] else {
            return None;
        };

        let stmt::Value::List(rows) = &input_values[position] else {
            return None;
        };

        // Every row binds the same number of variables.
        let Some(stmt::Value::Record(first)) = rows.first() else {
            return None;
        };
        let width = first.len();
        if width == 0 || rows.len() * width <= SQLITE_MAX_VARIABLES / 2 {
            return None;
        }

        // The variables the statement binds besides the batch: its constants,
        // and the other inputs wherever the statement reads them. Counted
        // from above (an input read in the statement is counted at every
        // read), so a chunk never lands over the limit.
        let mut others = scan.values;
        for (position_, occurrences) in scan.reads.iter().enumerate() {
            if position_ != position
                && let Some(value) = input_values.get(position_)
            {
                others += occurrences * bound_values(value);
            }
        }

        if others + rows.len() * width <= SQLITE_MAX_VARIABLES {
            return None;
        }

        let chunk_len = SQLITE_MAX_VARIABLES.checked_sub(others)? / width;
        if chunk_len == 0 {
            return None;
        }

        // `Value` has no `Hash`; its debug form is equal wherever two values
        // are.
        let mut seen = HashSet::new();
        let rows: Vec<_> = rows
            .iter()
            .filter(|row| seen.insert(format!("{row:?}")))
            .cloned()
            .collect();

        Some((
            position,
            rows.chunks(chunk_len)
                .map(<[stmt::Value]>::to_vec)
                .collect(),
        ))
    }
}

/// What a statement binds besides the rows of its batch input.
#[derive(Default)]
struct Scan {
    /// Positions of the `TableRef::Arg`s: the inputs read as a table.
    batch_positions: Vec<usize>,

    /// Constants, counted by the variables they bind.
    values: usize,

    /// How many times each input is read as an expression.
    reads: Vec<usize>,
}

impl Visit for Scan {
    fn visit_table_ref(&mut self, i: &stmt::TableRef) {
        match i {
            stmt::TableRef::Arg(expr_arg) => self.batch_positions.push(expr_arg.position),
            _ => stmt::visit::visit_table_ref(self, i),
        }
    }

    fn visit_expr_arg(&mut self, i: &stmt::ExprArg) {
        if self.reads.len() <= i.position {
            self.reads.resize(i.position + 1, 0);
        }
        self.reads[i.position] += 1;
    }

    fn visit_value(&mut self, i: &stmt::Value) {
        self.values += bound_values(i);
    }
}

/// An upper bound on the variables a value binds.
fn bound_values(value: &stmt::Value) -> usize {
    match value {
        stmt::Value::List(items) => items.iter().map(bound_values).sum(),
        stmt::Value::Record(record) => record.iter().map(bound_values).sum(),
        _ => 1,
    }
}
