//! Generated migrations applied to a SQLite database that already has rows.

use rusqlite::Connection;
use toasty_core::{
    driver::{Capability, Driver},
    schema::{
        db::{
            Column, ColumnId, Index, IndexColumn, IndexId, IndexOp, IndexScope, PrimaryKey, Schema,
            Table, TableId, Type,
        },
        diff,
    },
    stmt as core_stmt,
};
use toasty_driver_sqlite::Sqlite;
use toasty_sql::{Serializer, Statement};

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

/// Creates `schema` the way the driver does when it pushes a schema.
fn create_schema(schema: &Schema) -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    let serializer = Serializer::sqlite(schema);
    for table in &schema.tables {
        connection
            .execute(
                &serializer.serialize(&Statement::create_table(table, &Capability::SQLITE)),
                [],
            )
            .unwrap();
        for index in table.indices.iter().filter(|index| !index.primary_key) {
            connection
                .execute(&serializer.serialize(&Statement::create_index(index)), [])
                .unwrap();
        }
    }
    connection
}

/// Applies the driver's migration from `from` to `to` in one transaction, as
/// the driver applies a migration file.
fn migrate(
    connection: &Connection,
    from: &Schema,
    to: &Schema,
    hints: &diff::RenameHints,
) -> rusqlite::Result<()> {
    let diff = diff::Schema::from(from, to, hints);
    let migration = Sqlite::in_memory().generate_migration(&diff);

    connection.execute("BEGIN", [])?;
    for statement in migration.statements() {
        if let Err(e) = connection.execute(statement, []) {
            connection.execute("ROLLBACK", [])?;
            return Err(e);
        }
    }
    connection.execute("COMMIT", [])?;
    Ok(())
}

fn index_names(connection: &Connection, table: &str) -> Vec<String> {
    let mut stmt = connection
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'index' AND tbl_name = ?1 AND sql IS NOT NULL ORDER BY name",
        )
        .unwrap();
    stmt.query_map([table], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn count_rows(connection: &Connection, table: &str) -> i64 {
    connection
        .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn recreate_table_keeps_unchanged_indices_on_a_filled_table() {
    let indices = || {
        vec![
            make_index(0, 1, "index_users_by_email", &[1], true),
            make_index(0, 2, "index_users_by_name", &[2], false),
        ]
    };
    let from = users(true, indices());
    let to = users(false, indices());

    let connection = create_schema(&from);
    connection
        .execute(
            "INSERT INTO users (id, email, name) VALUES (1, 'a@example.com', 'a'), (2, 'b@example.com', 'b')",
            [],
        )
        .unwrap();

    migrate(&connection, &from, &to, &diff::RenameHints::new()).unwrap();

    assert_eq!(
        index_names(&connection, "users"),
        ["index_users_by_email", "index_users_by_name"]
    );
    assert_eq!(count_rows(&connection, "users"), 2);

    let duplicate = connection.execute(
        "INSERT INTO users (id, email, name) VALUES (3, 'a@example.com', 'c')",
        [],
    );
    assert!(
        duplicate
            .unwrap_err()
            .to_string()
            .contains("UNIQUE constraint failed: users.email"),
    );
}

#[test]
fn recreate_table_creates_new_and_altered_indices_once_on_a_filled_table() {
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

    let connection = create_schema(&from);
    connection
        .execute(
            "INSERT INTO users (id, email, name) VALUES (1, 'a@example.com', 'a'), (2, 'b@example.com', 'b')",
            [],
        )
        .unwrap();

    migrate(&connection, &from, &to, &diff::RenameHints::new()).unwrap();

    assert_eq!(
        index_names(&connection, "users"),
        [
            "index_users_by_email",
            "index_users_by_email_and_name",
            "index_users_by_name",
        ]
    );
    assert_eq!(count_rows(&connection, "users"), 2);

    // `index_users_by_name` is no longer unique.
    connection
        .execute(
            "INSERT INTO users (id, email, name) VALUES (3, 'c@example.com', 'a')",
            [],
        )
        .unwrap();
}

#[test]
fn rename_table_applies_to_a_filled_table() {
    let table = |name: &str| Schema {
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
    };
    let from = table("users");
    let to = table("accounts");
    let mut hints = diff::RenameHints::new();
    hints.add_table_hint(TableId(0), TableId(0));

    let migration = Sqlite::in_memory().generate_migration(&diff::Schema::from(&from, &to, &hints));
    assert!(
        !migration.statements().iter().any(|s| s.contains("_by_id")),
        "got: {migration:#?}"
    );

    let connection = create_schema(&from);
    connection
        .execute(
            "INSERT INTO users (id, email) VALUES (1, 'a@example.com'), (2, 'b@example.com')",
            [],
        )
        .unwrap();

    migrate(&connection, &from, &to, &hints).unwrap();

    assert_eq!(count_rows(&connection, "accounts"), 2);
    assert_eq!(
        index_names(&connection, "accounts"),
        ["index_accounts_by_email"]
    );
    assert!(
        connection
            .execute(
                "INSERT INTO accounts (id, email) VALUES (3, 'a@example.com')",
                [],
            )
            .is_err()
    );
}

fn users_with_name(name_nullable: bool) -> Schema {
    let mut name = make_column(0, 1, "name", Type::Text);
    name.nullable = name_nullable;
    Schema {
        tables: vec![make_table(
            0,
            "users",
            vec![make_column(0, 0, "id", Type::Integer(8)), name],
            vec![],
        )],
    }
}

fn users_without_name() -> Schema {
    Schema {
        tables: vec![make_table(
            0,
            "users",
            vec![make_column(0, 0, "id", Type::Integer(8))],
            vec![],
        )],
    }
}

#[test]
fn sqlite_refuses_add_column_not_null_only_once_the_table_has_rows() {
    // Why a migration that adds a NOT NULL column cannot be tested on an
    // empty table.
    let connection = create_schema(&users_without_name());
    connection
        .execute("ALTER TABLE users ADD COLUMN a TEXT NOT NULL", [])
        .unwrap();

    connection
        .execute("INSERT INTO users (id, a) VALUES (1, 'a')", [])
        .unwrap();
    let err = connection
        .execute("ALTER TABLE users ADD COLUMN b TEXT NOT NULL", [])
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Cannot add a NOT NULL column with default value NULL"),
        "got: {err}"
    );
}

#[test]
#[should_panic(
    expected = "cannot add the column \"users\".\"name\": SQLite cannot add a NOT NULL column without a default value"
)]
fn add_column_not_null_is_rejected_before_it_reaches_a_filled_table() {
    let from = users_without_name();
    let connection = create_schema(&from);
    connection
        .execute("INSERT INTO users (id) VALUES (1), (2)", [])
        .unwrap();

    let _ = migrate(
        &connection,
        &from,
        &users_with_name(false),
        &diff::RenameHints::new(),
    );
}

#[test]
fn add_column_nullable_applies_to_a_filled_table() {
    let from = users_without_name();
    let connection = create_schema(&from);
    connection
        .execute("INSERT INTO users (id) VALUES (1), (2)", [])
        .unwrap();

    migrate(
        &connection,
        &from,
        &users_with_name(true),
        &diff::RenameHints::new(),
    )
    .unwrap();

    let nulls: i64 = connection
        .query_row("SELECT COUNT(*) FROM users WHERE name IS NULL", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(nulls, 2);
}

#[test]
fn applies_a_migration_with_a_comment_only_chunk() {
    use toasty_core::driver::{ConnectContext, Driver};
    use toasty_core::schema::db::Migration;

    let migration = Migration::new_sql(
        "CREATE TABLE notes (id INTEGER PRIMARY KEY)\n\
         -- #[toasty::breakpoint]\n\
         -- a note written by hand\n\
         \n\
         -- #[toasty::breakpoint]\n\
         CREATE INDEX notes_by_id ON notes (id)\n"
            .to_string(),
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut connection = Sqlite::in_memory()
            .connect(&ConnectContext::default())
            .await
            .unwrap();
        connection
            .apply_migration(1, "0001_notes.sql", &migration)
            .await
            .unwrap();

        let applied = connection.applied_migrations().await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].id(), 1);
    });
}
