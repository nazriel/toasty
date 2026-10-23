use toasty_core::{
    driver::Capability,
    schema::{
        db::{
            Column, ColumnId, Index, IndexColumn, IndexId, IndexOp, IndexScope, PrimaryKey, Schema,
            Table, TableId, Type,
        },
        diff,
    },
    stmt as core_stmt,
};
use toasty_sql::{Serializer, migration::MigrationStatement};

fn make_column(table_id: usize, index: usize, name: &str, storage_ty: Type) -> Column {
    Column {
        id: ColumnId {
            table: TableId(table_id),
            index,
        },
        name: name.to_string(),
        ty: core_stmt::Type::String,
        storage_ty,
        nullable: false,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    }
}

fn make_index(table_id: usize, index: usize, name: &str, columns: &[usize], unique: bool) -> Index {
    Index {
        id: IndexId {
            table: TableId(table_id),
            index,
        },
        name: name.to_string(),
        on: TableId(table_id),
        columns: columns
            .iter()
            .map(|&column| IndexColumn {
                column: ColumnId {
                    table: TableId(table_id),
                    index: column,
                },
                op: IndexOp::Eq,
                scope: IndexScope::Local,
            })
            .collect(),
        unique,
        primary_key: false,
        filter: None,
    }
}

/// A table whose first column is the primary key, with the primary key's
/// index first in `indices`, as the schema builder lays them out.
fn make_table(id: usize, name: &str, columns: Vec<Column>, indices: Vec<Index>) -> Table {
    let pk_index = Index {
        primary_key: true,
        ..make_index(id, 0, &format!("index_{name}_by_id"), &[0], true)
    };

    Table {
        id: TableId(id),
        schema: None,
        name: name.to_string(),
        columns,
        primary_key: PrimaryKey {
            columns: vec![ColumnId {
                table: TableId(id),
                index: 0,
            }],
            index: pk_index.id,
        },
        indices: std::iter::once(pk_index).chain(indices).collect(),
    }
}

fn serialize_migration(stmts: &[MigrationStatement<'_>], flavor: &str) -> Vec<String> {
    stmts
        .iter()
        .map(|ms| {
            let serializer = match flavor {
                "sqlite" => Serializer::sqlite(ms.schema()),
                "postgresql" => Serializer::postgresql(ms.schema()),
                "mysql" => Serializer::mysql(ms.schema()),
                _ => panic!("unknown flavor: {flavor}"),
            };
            serializer.serialize(ms.statement())
        })
        .collect()
}

/// `<name> (id, email)` with a unique index on `email` named after the table.
fn schema(name: &str) -> Schema {
    Schema {
        tables: vec![make_table(
            0,
            name,
            vec![
                make_column(0, 0, "id", Type::Integer(8)),
                make_column(0, 1, "email", Type::Text),
            ],
            vec![make_index(
                0,
                1,
                &format!("index_{name}_by_email"),
                &[1],
                true,
            )],
        )],
    }
}

fn table_hint() -> diff::RenameHints {
    let mut hints = diff::RenameHints::new();
    hints.add_table_hint(TableId(0), TableId(0));
    hints
}

fn rename_users_to_accounts(
    hints: &diff::RenameHints,
    capability: &Capability,
    flavor: &str,
) -> Vec<String> {
    let from = schema("users");
    let to = schema("accounts");
    let diff = diff::Schema::from(&from, &to, hints);
    serialize_migration(&MigrationStatement::from_diff(&diff, capability), flavor)
}

#[test]
fn rename_table_does_not_drop_or_create_primary_key_index() {
    for (capability, flavor) in [
        (&Capability::SQLITE, "sqlite"),
        (&Capability::POSTGRESQL, "postgresql"),
        (&Capability::MYSQL, "mysql"),
    ] {
        let sql = rename_users_to_accounts(&table_hint(), capability, flavor);

        assert!(
            !sql.iter().any(|s| s.contains("_by_id")),
            "{flavor}: got: {sql:#?}"
        );
    }
}

#[test]
fn rename_table_sqlite_renames_secondary_index() {
    let sql = rename_users_to_accounts(&table_hint(), &Capability::SQLITE, "sqlite");

    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"users\" RENAME TO \"accounts\";",
            "DROP INDEX \"index_users_by_email\";",
            "CREATE UNIQUE INDEX \"index_accounts_by_email\" ON \"accounts\" (\"email\");",
        ]
    );
}

#[test]
fn rename_table_sqlite_with_index_hints_renames_secondary_index() {
    // Answering "rename" for the indices turns their drop and create into
    // alters; the primary key's index still produces no statement.
    let mut hints = table_hint();
    for index in 0..2 {
        let id = IndexId {
            table: TableId(0),
            index,
        };
        hints.add_index_hint(id, id);
    }

    let sql = rename_users_to_accounts(&hints, &Capability::SQLITE, "sqlite");

    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"users\" RENAME TO \"accounts\";",
            "DROP INDEX \"index_users_by_email\";",
            "CREATE UNIQUE INDEX \"index_accounts_by_email\" ON \"accounts\" (\"email\");",
        ]
    );
}

#[test]
fn rename_table_postgresql_renames_secondary_index() {
    let sql = rename_users_to_accounts(&table_hint(), &Capability::POSTGRESQL, "postgresql");

    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"users\" RENAME TO \"accounts\";",
            "DROP INDEX \"index_users_by_email\";",
            "CREATE UNIQUE INDEX \"index_accounts_by_email\" ON \"accounts\" (\"email\");",
        ]
    );
}
