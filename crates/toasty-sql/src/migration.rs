use std::{borrow::Cow, collections::HashSet};

use toasty_core::{
    driver::{Capability, Dialect},
    schema::{
        db::{Column, Schema, Table, TableId, Type, TypeEnum},
        diff,
    },
};

use crate::stmt::{AlterColumnChanges, AlterTable, AlterTableAction, DropTable, Name, Statement};

/// Returns `true` if a named enum change is handled at the type level and
/// should not produce column-level DDL.
fn is_named_enum_type_only_change(
    previous: &Column,
    next: &Column,
    renamed_types: &HashSet<(TypeKey<'_>, TypeKey<'_>)>,
) -> bool {
    if previous.name != next.name
        || previous.nullable != next.nullable
        || previous.primary_key != next.primary_key
        || previous.auto_increment != next.auto_increment
    {
        return false;
    }

    // A scalar ↔ array change is a real column type change, so both sides
    // must have the same shape.
    let same_shape = matches!(
        (&previous.storage_ty, &next.storage_ty),
        (Type::Enum(_), Type::Enum(_)) | (Type::List(_), Type::List(_))
    );

    if !same_shape {
        return false;
    }

    match (
        previous.storage_ty.named_enum().and_then(type_key),
        next.storage_ty.named_enum().and_then(type_key),
    ) {
        (Some(a), Some(b)) => a == b || renamed_types.contains(&(a, b)),
        _ => false,
    }
}

/// A named enum type's identity: its schema and its name.
type TypeKey<'a> = (Option<&'a str>, &'a str);

fn type_key(ty: &TypeEnum) -> Option<TypeKey<'_>> {
    Some((ty.schema.as_deref(), ty.name.as_deref()?))
}

/// Panics if a table or an enum type moves to another database schema
/// between the previous and the next snapshot.
///
/// On PostgreSQL that is `ALTER ... SET SCHEMA`, and whether the objects that
/// refer to it should move along is a decision the schema does not record.
/// The only correct migration is one the user writes.
fn assert_schema_unchanged(kind: &str, name: &str, previous: Option<&str>, next: Option<&str>) {
    if previous != next {
        let describe = |schema: Option<&str>| match schema {
            Some(schema) => format!("schema \"{schema}\""),
            None => "the default schema".to_string(),
        };
        panic!(
            "cannot move the {kind} \"{name}\" from {} to {}: moving a {kind} to another \
             schema is not generated. Write this migration by hand.",
            describe(previous),
            describe(next),
        );
    }
}

/// Panics if `column` cannot be added to `table` on a database that already
/// has rows in it.
///
/// SQLite refuses `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a non-null
/// `DEFAULT` once the table has a row, and a table rebuild fails the same way
/// when it copies those rows. The schema carries no column defaults, so the
/// only correct migration is one the user writes. Auto-increment columns are
/// left to the serializer.
fn assert_column_can_be_added(table: &Table, column: &Column, capability: &Capability) {
    if capability.sql == Some(Dialect::Sqlite) && !column.nullable && !column.auto_increment {
        panic!(
            "cannot add the column \"{}\".\"{}\": SQLite cannot add a NOT NULL column without a \
             default value to a table that has rows. Add the column as nullable (an `Option` \
             field), or write this migration by hand and give the column a non-null DEFAULT.",
            table.name, column.name,
        );
    }
}

/// A migration step pairing a DDL [`Statement`] with the [`Schema`] it applies against.
///
/// Each `MigrationStatement` carries a snapshot of the schema at the point where
/// the statement should be serialized. This is necessary because rename and
/// recreation operations modify the schema as they go.
pub struct MigrationStatement<'a> {
    statement: Statement,
    schema: Cow<'a, Schema>,
}

impl<'a> MigrationStatement<'a> {
    fn new(statement: Statement, schema: Cow<'a, Schema>) -> Self {
        MigrationStatement { statement, schema }
    }

    /// Generates migration statements from a [`diff::Schema`].
    ///
    /// Walks the diff's type, table, column, and index changes and produces
    /// the corresponding DDL statements. Type changes (CREATE TYPE, ALTER
    /// TYPE) are emitted before table changes. On databases that lack
    /// `ALTER COLUMN` support (e.g. SQLite), column type changes trigger a
    /// full table recreation sequence.
    pub fn from_diff(schema_diff: &'a diff::Schema<'a>, capability: &Capability) -> Vec<Self> {
        let mut result = Vec::new();
        let types_diff = schema_diff.types();
        let renamed_types: HashSet<(TypeKey<'_>, TypeKey<'_>)> = types_diff
            .iter()
            .filter_map(|item| match item {
                diff::Type::Rename { previous, next } => Some((
                    type_key(previous).expect("named enum type"),
                    type_key(next).expect("named enum type"),
                )),
                _ => None,
            })
            .collect();

        for item in types_diff.iter() {
            if let diff::Type::Rename { previous, next } = item {
                assert_schema_unchanged(
                    "enum type",
                    previous.name.as_deref().expect("named enum type"),
                    previous.schema.as_deref(),
                    next.schema.as_deref(),
                );
            }
        }
        for table in schema_diff.tables().iter() {
            if let diff::Table::Alter { previous, next, .. } = table {
                assert_schema_unchanged(
                    "table",
                    &previous.name,
                    previous.schema.as_deref(),
                    next.schema.as_deref(),
                );
            }
        }

        // Emit enum type changes before table changes (tables may reference
        // newly created types).
        if capability.named_enum_types {
            for item in types_diff.iter() {
                match item {
                    diff::Type::Create(ty) => {
                        result.push(Self::new(
                            Statement::create_enum_type(ty),
                            Cow::Borrowed(schema_diff.next()),
                        ));
                    }
                    diff::Type::Rename { previous, next } => {
                        result.push(Self::new(
                            Statement::rename_type(
                                previous,
                                next.name.as_deref().expect("named enum type"),
                            ),
                            Cow::Borrowed(schema_diff.previous()),
                        ));
                    }
                    diff::Type::AddVariants { ty, added } => {
                        for variant in added {
                            result.push(Self::new(
                                Statement::alter_type_add_value(ty, variant),
                                Cow::Borrowed(schema_diff.next()),
                            ));
                        }
                    }
                }
            }
        }

        for table in schema_diff.tables().iter() {
            match table {
                diff::Table::Create(table) => {
                    result.push(Self::new(
                        Statement::create_table(table, capability),
                        Cow::Borrowed(schema_diff.next()),
                    ));
                    for index in &table.indices {
                        if index.primary_key {
                            continue; // PK indices are created as part of CREATE TABLE
                        }
                        result.push(Self::new(
                            Statement::create_index(index),
                            Cow::Borrowed(schema_diff.next()),
                        ));
                    }
                }
                diff::Table::Drop(table) => result.push(Self::new(
                    Statement::drop_table(table),
                    Cow::Borrowed(schema_diff.previous()),
                )),
                diff::Table::Alter {
                    previous,
                    next,
                    columns,
                    indices,
                    ..
                } => {
                    for item in columns.iter() {
                        if let diff::Column::Add(column) = item {
                            assert_column_can_be_added(next, column, capability);
                        }
                    }

                    let mut schema = Cow::Borrowed(schema_diff.previous());
                    if previous.name != next.name {
                        result.push(Self::new(
                            Statement::alter_table_rename_to(previous, &next.name),
                            schema.clone(),
                        ));
                        schema.to_mut().table_mut(previous.id).name = next.name.clone();
                    }

                    // Check if any column alteration requires table recreation
                    // (e.g. SQLite can't alter column type/nullability/auto_increment)
                    let needs_recreation = !capability.schema_mutations.alter_column_type
                        && columns.iter().any(|item| {
                            matches!(
                                item,
                                diff::Column::Alter {
                                    previous: prev_col,
                                    next: next_col
                                } if AlterColumnChanges::from_diff(prev_col, next_col).has_type_change()
                                    && !(capability.named_enum_types
                                        && is_named_enum_type_only_change(
                                            prev_col,
                                            next_col,
                                            &renamed_types,
                                        ))
                            )
                        });

                    // Drop existing indices before changing columns because they may
                    // reference a column that is being removed or altered. PK indices
                    // are part of CREATE TABLE rather than separate indices, so there
                    // is nothing to drop, even when a table rename changes their name.
                    for item in indices.iter() {
                        match item {
                            diff::Index::Drop(index) if !index.primary_key => {
                                result.push(Self::new(
                                    Statement::drop_index(previous, index),
                                    Cow::Borrowed(schema_diff.previous()),
                                ));
                            }
                            diff::Index::Alter {
                                previous: previous_index,
                                ..
                            } if !previous_index.primary_key => {
                                result.push(Self::new(
                                    Statement::drop_index(previous, previous_index),
                                    Cow::Borrowed(schema_diff.previous()),
                                ));
                            }
                            _ => {}
                        }
                    }

                    if needs_recreation {
                        Self::emit_table_recreation(
                            &mut result,
                            schema,
                            schema_diff.next(),
                            previous,
                            next,
                            columns,
                            capability,
                        );
                        // The recreation creates every index of the next table,
                        // including the ones this diff creates or alters.
                        continue;
                    }

                    Self::emit_column_changes(
                        &mut result,
                        schema,
                        previous.id,
                        columns,
                        capability,
                        &renamed_types,
                    );

                    // Create new indices after the column changes so they reference
                    // the next schema's column definitions. PK indices are created as
                    // part of CREATE TABLE.
                    for item in indices.iter() {
                        match item {
                            diff::Index::Create(index) if !index.primary_key => {
                                result.push(Self::new(
                                    Statement::create_index(index),
                                    Cow::Borrowed(schema_diff.next()),
                                ));
                            }
                            diff::Index::Alter { next, .. } if !next.primary_key => {
                                result.push(Self::new(
                                    Statement::create_index(next),
                                    Cow::Borrowed(schema_diff.next()),
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        result
    }

    fn emit_table_recreation(
        result: &mut Vec<Self>,
        schema: Cow<'a, Schema>,
        next_schema: &'a Schema,
        previous: &Table,
        next: &Table,
        columns: &[diff::Column<'_>],
        capability: &Capability,
    ) {
        let current_name = schema.table(previous.id).name.clone();
        let temp_name = format!("_toasty_new_{}", current_name);

        // 1. PRAGMA foreign_keys = OFF
        result.push(Self::new(
            Statement::pragma_disable_foreign_keys(),
            schema.clone(),
        ));

        // 2. CREATE TABLE temp with new schema
        let temp_schema = {
            let mut s = schema.as_ref().clone();
            let t = s.table_mut(next.id);
            t.name = temp_name.clone();
            t.columns = next.columns.clone();
            t.primary_key = next.primary_key.clone();
            s
        };
        result.push(Self::new(
            Statement::create_table(next, capability),
            Cow::Owned(temp_schema),
        ));

        // 3. INSERT INTO temp SELECT ... FROM current
        let column_mappings: Vec<(Name, Name)> = next
            .columns
            .iter()
            .filter(|col| {
                // Skip added columns (no source data)
                !columns
                    .iter()
                    .any(|item| matches!(item, diff::Column::Add(c) if c.id == col.id))
            })
            .map(|col| {
                let target_name = Name::from(&col.name[..]);
                // Check if this column was renamed
                let source_name = columns
                    .iter()
                    .find_map(|item| match item {
                        diff::Column::Alter {
                            previous: prev_col,
                            next: next_col,
                        } if next_col.id == col.id && prev_col.name != next_col.name => {
                            Some(Name::from(&prev_col.name[..]))
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| Name::from(&col.name[..]));
                (target_name, source_name)
            })
            .collect();

        result.push(Self::new(
            Statement::copy_table(
                Name::from(current_name.as_str()),
                Name::from(temp_name.as_str()),
                column_mappings,
            ),
            schema.clone(),
        ));

        // 4. DROP TABLE current
        result.push(Self::new(
            DropTable {
                name: Name::from(current_name.as_str()),
                if_exists: false,
            }
            .into(),
            schema.clone(),
        ));

        // 5. ALTER TABLE temp RENAME TO current
        result.push(Self::new(
            AlterTable {
                name: Name::from(temp_name.as_str()),
                action: AlterTableAction::RenameTo(Name::from(current_name.as_str())),
            }
            .into(),
            schema.clone(),
        ));

        // 6. CREATE INDEX for every index of the next table. Dropping the
        // current table dropped all of its indices, not only the ones the diff
        // changes.
        for index in &next.indices {
            if index.primary_key {
                continue; // PK indices are created as part of CREATE TABLE
            }
            result.push(Self::new(
                Statement::create_index(index),
                Cow::Borrowed(next_schema),
            ));
        }

        // 7. PRAGMA foreign_keys = ON
        result.push(Self::new(
            Statement::pragma_enable_foreign_keys(),
            schema.clone(),
        ));
    }

    fn emit_column_changes(
        result: &mut Vec<Self>,
        schema: Cow<'a, Schema>,
        table: TableId,
        columns: &[diff::Column<'_>],
        capability: &Capability,
        renamed_types: &HashSet<(TypeKey<'_>, TypeKey<'_>)>,
    ) {
        for item in columns.iter() {
            match item {
                diff::Column::Add(column) => {
                    result.push(Self::new(
                        Statement::add_column(table, column, capability),
                        schema.clone(),
                    ));
                }
                diff::Column::Drop(column) => {
                    result.push(Self::new(Statement::drop_column(column), schema.clone()));
                }
                diff::Column::Alter {
                    previous,
                    next: col_next,
                } => {
                    // Skip column-level DDL for named enum changes handled by
                    // ALTER TYPE.
                    if capability.named_enum_types
                        && is_named_enum_type_only_change(previous, col_next, renamed_types)
                    {
                        continue;
                    }

                    let changes = AlterColumnChanges::from_diff(previous, col_next);
                    let changes = if capability.schema_mutations.alter_column_properties_atomic {
                        vec![changes]
                    } else {
                        changes.split()
                    };

                    for changes in changes {
                        result.push(Self::new(
                            Statement::alter_column(previous, changes, capability),
                            schema.clone(),
                        ));
                    }
                }
            }
        }
    }

    /// Returns the DDL statement for this migration step.
    pub fn statement(&self) -> &Statement {
        &self.statement
    }

    /// Returns the schema snapshot this statement should be serialized against.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }
}
