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

fn serialize_migration(stmts: &[MigrationStatement<'_>]) -> Vec<String> {
    stmts
        .iter()
        .map(|ms| Serializer::sqlite(ms.schema()).serialize(ms.statement()))
        .collect()
}

/// `users (id, email, name)`; `email`'s nullability is what the tests change
/// to force a rebuild.
fn users(email_nullable: bool, indices: Vec<Index>) -> Schema {
    let mut email = make_column(0, 1, "email", Type::Text);
    email.nullable = email_nullable;
    Schema {
        tables: vec![make_table(
            0,
            "users",
            vec![
                make_column(0, 0, "id", Type::Integer(8)),
                email,
                make_column(0, 2, "name", Type::Text),
            ],
            indices,
        )],
    }
}

fn sqlite_migration(from: &Schema, to: &Schema, hints: &diff::RenameHints) -> Vec<String> {
    let diff = diff::Schema::from(from, to, hints);
    serialize_migration(&MigrationStatement::from_diff(&diff, &Capability::SQLITE))
}

#[test]
fn recreate_table_sqlite_keeps_unchanged_indices() {
    // email: nullable → not null rebuilds the table; neither index changes.
    let indices = || {
        vec![
            make_index(0, 1, "index_users_by_email", &[1], true),
            make_index(0, 2, "index_users_by_name", &[2], false),
        ]
    };

    let sql = sqlite_migration(
        &users(true, indices()),
        &users(false, indices()),
        &diff::RenameHints::new(),
    );

    assert_eq!(
        sql,
        vec![
            "PRAGMA foreign_keys = OFF;",
            "CREATE TABLE \"_toasty_new_users\" (\n    \"id\" BIGINT NOT NULL,\n    \"email\" TEXT NOT NULL,\n    \"name\" TEXT NOT NULL,\n    PRIMARY KEY (\"id\")\n);",
            "INSERT INTO \"_toasty_new_users\" (\"id\", \"email\", \"name\") SELECT \"id\", \"email\", \"name\" FROM \"users\";",
            "DROP TABLE \"users\";",
            "ALTER TABLE \"_toasty_new_users\" RENAME TO \"users\";",
            "CREATE UNIQUE INDEX \"index_users_by_email\" ON \"users\" (\"email\");",
            "CREATE INDEX \"index_users_by_name\" ON \"users\" (\"name\");",
            "PRAGMA foreign_keys = ON;",
        ]
    );
}

#[test]
fn recreate_table_sqlite_creates_new_and_altered_indices_once() {
    // The rebuild comes with one index altered (no longer unique) and one
    // created; a third index is unchanged.
    let from = users(
        true,
        vec![
            make_index(0, 1, "index_users_by_email", &[1], true),
            make_index(0, 2, "index_users_by_name", &[2], true),
        ],
    );
    let to = users(
        false,
        vec![
            make_index(0, 1, "index_users_by_email", &[1], true),
            make_index(0, 2, "index_users_by_name", &[2], false),
            make_index(0, 3, "index_users_by_email_and_name", &[1, 2], false),
        ],
    );

    let sql = sqlite_migration(&from, &to, &diff::RenameHints::new());

    // The altered index is dropped before the rebuild, while the old table
    // still has it; every index of the next table is created once, after it.
    assert_eq!(
        sql,
        vec![
            "DROP INDEX \"index_users_by_name\";",
            "PRAGMA foreign_keys = OFF;",
            "CREATE TABLE \"_toasty_new_users\" (\n    \"id\" BIGINT NOT NULL,\n    \"email\" TEXT NOT NULL,\n    \"name\" TEXT NOT NULL,\n    PRIMARY KEY (\"id\")\n);",
            "INSERT INTO \"_toasty_new_users\" (\"id\", \"email\", \"name\") SELECT \"id\", \"email\", \"name\" FROM \"users\";",
            "DROP TABLE \"users\";",
            "ALTER TABLE \"_toasty_new_users\" RENAME TO \"users\";",
            "CREATE UNIQUE INDEX \"index_users_by_email\" ON \"users\" (\"email\");",
            "CREATE INDEX \"index_users_by_name\" ON \"users\" (\"name\");",
            "CREATE INDEX \"index_users_by_email_and_name\" ON \"users\" (\"email\", \"name\");",
            "PRAGMA foreign_keys = ON;",
        ]
    );
}

#[test]
fn recreate_table_sqlite_does_not_recreate_dropped_index() {
    let from = users(
        true,
        vec![
            make_index(0, 1, "index_users_by_email", &[1], true),
            make_index(0, 2, "index_users_by_name", &[2], false),
        ],
    );
    let to = users(
        false,
        vec![make_index(0, 1, "index_users_by_email", &[1], true)],
    );

    let sql = sqlite_migration(&from, &to, &diff::RenameHints::new());

    assert_eq!(sql[0], "DROP INDEX \"index_users_by_name\";");
    assert_eq!(
        sql.iter()
            .filter(|s| s.contains("index_users_by_name"))
            .count(),
        1,
        "got: {sql:#?}"
    );
    assert!(
        sql.iter()
            .any(|s| s == "CREATE UNIQUE INDEX \"index_users_by_email\" ON \"users\" (\"email\");"),
        "got: {sql:#?}"
    );
}

#[test]
fn recreate_table_sqlite_with_table_rename_creates_indices_under_new_name() {
    let from = users(
        true,
        vec![make_index(0, 1, "index_users_by_email", &[1], true)],
    );
    let mut to = users(
        false,
        vec![make_index(0, 1, "index_accounts_by_email", &[1], true)],
    );
    to.tables[0].name = "accounts".to_string();
    to.tables[0].indices[0].name = "index_accounts_by_id".to_string();

    let mut hints = diff::RenameHints::new();
    hints.add_table_hint(TableId(0), TableId(0));
    let sql = sqlite_migration(&from, &to, &hints);

    // SQLite keeps an index's name when its table is renamed, so the old
    // name is the one to drop.
    assert_eq!(sql[0], "ALTER TABLE \"users\" RENAME TO \"accounts\";");
    assert!(
        sql.iter()
            .any(|s| s == "DROP INDEX \"index_users_by_email\";"),
        "got: {sql:#?}"
    );
    assert_eq!(
        sql.iter()
            .filter(|s| {
                *s == "CREATE UNIQUE INDEX \"index_accounts_by_email\" ON \"accounts\" (\"email\");"
            })
            .count(),
        1,
        "got: {sql:#?}"
    );
    assert_eq!(sql.last().unwrap(), "PRAGMA foreign_keys = ON;");
}
