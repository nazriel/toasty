//! Schemas and helpers shared by the tests of the migration commands.

use toasty::schema::{
    db::{
        Column, ColumnId, Index, IndexColumn, IndexId, IndexOp, IndexScope, PrimaryKey, Schema,
        Table, TableId, Type,
    },
    diff,
};
use toasty_core::stmt;

fn column(table: usize, index: usize, name: &str) -> Column {
    Column {
        id: ColumnId {
            table: TableId(table),
            index,
        },
        name: name.to_string(),
        ty: stmt::Type::String,
        storage_ty: if index == 0 {
            Type::Integer(8)
        } else {
            Type::Text
        },
        nullable: false,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    }
}

fn index(table: usize, position: usize, name: &str, column: usize, primary_key: bool) -> Index {
    Index {
        id: IndexId {
            table: TableId(table),
            index: position,
        },
        name: name.to_string(),
        on: TableId(table),
        columns: vec![IndexColumn {
            column: ColumnId {
                table: TableId(table),
                index: column,
            },
            op: IndexOp::Eq,
            scope: IndexScope::Local,
        }],
        unique: primary_key,
        primary_key,
    }
}

/// A table with an `id` primary key, one column per name in `columns`, and one
/// index per name in `indices`, each on the column at the same position.
pub(crate) fn table(id: usize, name: &str, columns: &[&str], indices: &[&str]) -> Table {
    let primary_key = index(id, 0, &format!("index_{name}_by_id"), 0, true);
    Table {
        id: TableId(id),
        name: name.to_string(),
        columns: std::iter::once("id")
            .chain(columns.iter().copied())
            .enumerate()
            .map(|(position, column_name)| column(id, position, column_name))
            .collect(),
        primary_key: PrimaryKey {
            columns: vec![ColumnId {
                table: TableId(id),
                index: 0,
            }],
            index: primary_key.id,
        },
        indices: std::iter::once(primary_key)
            .chain(indices.iter().enumerate().map(|(position, index_name)| {
                index(id, position + 1, index_name, position + 1, false)
            }))
            .collect(),
    }
}

pub(crate) fn schema(tables: Vec<Table>) -> Schema {
    Schema { tables }
}

/// Describes what the diff from `previous` to `next` does, one line per
/// change.
pub(crate) fn describe(previous: &Schema, next: &Schema, hints: &diff::RenameHints) -> Vec<String> {
    let diff = diff::Schema::from(previous, next, hints);
    let mut lines = Vec::new();
    for item in diff.tables().iter() {
        match item {
            diff::Table::Create(table) => lines.push(format!("create table {}", table.name)),
            diff::Table::Drop(table) => lines.push(format!("drop table {}", table.name)),
            diff::Table::Alter {
                previous,
                next,
                columns,
                indices,
            } => {
                if previous.name != next.name {
                    lines.push(format!("rename table {} to {}", previous.name, next.name));
                }
                for column in columns {
                    match column {
                        diff::Column::Add(c) => {
                            lines.push(format!("add column {}.{}", next.name, c.name))
                        }
                        diff::Column::Drop(c) => {
                            lines.push(format!("drop column {}.{}", previous.name, c.name))
                        }
                        diff::Column::Alter {
                            previous: a,
                            next: b,
                        } if a.name != b.name => lines.push(format!(
                            "rename column {}.{} to {}",
                            previous.name, a.name, b.name
                        )),
                        diff::Column::Alter { .. } => {}
                    }
                }
                for index in indices {
                    match index {
                        diff::Index::Create(i) => {
                            lines.push(format!("create index {}.{}", next.name, i.name))
                        }
                        diff::Index::Drop(i) => {
                            lines.push(format!("drop index {}.{}", previous.name, i.name))
                        }
                        diff::Index::Alter {
                            previous: a,
                            next: b,
                        } if a.name != b.name => lines.push(format!(
                            "rename index {}.{} to {}",
                            previous.name, a.name, b.name
                        )),
                        diff::Index::Alter { .. } => {}
                    }
                }
            }
        }
    }
    lines
}
