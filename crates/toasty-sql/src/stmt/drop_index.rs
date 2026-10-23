use super::{Name, Statement};

use toasty_core::schema::db::{Index, Table};

/// A statement to drop a SQL index.
#[derive(Debug, Clone)]
pub struct DropIndex {
    /// Name of the index.
    pub name: Name,

    /// Whether or not to add an `IF EXISTS` clause.
    pub if_exists: bool,
}

impl Statement {
    /// Drops an index of `table`. The index is named in its table's schema.
    ///
    /// This function _does not_ add an `IF EXISTS` clause.
    pub fn drop_index(table: &Table, index: &Index) -> Self {
        DropIndex {
            name: Name::qualified(table.schema.as_deref(), &index.name),
            if_exists: false,
        }
        .into()
    }

    /// Drops a index of `table` if it exists. The index is named in its
    /// table's schema.
    ///
    /// This function _does_ add an `IF EXISTS` clause.
    pub fn drop_index_if_exists(table: &Table, index: &Index) -> Self {
        DropIndex {
            name: Name::qualified(table.schema.as_deref(), &index.name),
            if_exists: true,
        }
        .into()
    }
}

impl From<DropIndex> for Statement {
    fn from(value: DropIndex) -> Self {
        Self::DropIndex(value)
    }
}
