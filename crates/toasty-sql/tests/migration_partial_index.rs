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

fn column(index: usize, name: &str, storage_ty: Type, nullable: bool) -> Column {
    Column {
        id: ColumnId {
            table: TableId(0),
            index,
        },
        name: name.to_string(),
        ty: core_stmt::Type::String,
        storage_ty,
        nullable,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    }
}

/// A `tokens` table with a unique index on `(owner, name)`, partial when
/// `filter` is set.
fn schema(filter: Option<&str>) -> Schema {
    let index_column = |index| IndexColumn {
        column: ColumnId {
            table: TableId(0),
            index,
        },
        op: IndexOp::Eq,
        scope: IndexScope::Local,
    };

    Schema {
        tables: vec![Table {
            id: TableId(0),
            schema: None,
            name: "tokens".to_string(),
            columns: vec![
                column(0, "id", Type::Integer(8), false),
                column(1, "owner", Type::Integer(8), false),
                column(2, "name", Type::Text, false),
                column(3, "revoked_at", Type::Integer(8), true),
            ],
            primary_key: PrimaryKey {
                columns: vec![ColumnId {
                    table: TableId(0),
                    index: 0,
                }],
                index: IndexId {
                    table: TableId(0),
                    index: 0,
                },
            },
            indices: vec![Index {
                id: IndexId {
                    table: TableId(0),
                    index: 0,
                },
                name: "index_tokens_by_owner_and_name".to_string(),
                on: TableId(0),
                columns: vec![index_column(1), index_column(2)],
                unique: true,
                primary_key: false,
                filter: filter.map(str::to_string),
            }],
        }],
    }
}

fn migrate(from: &Schema, to: &Schema, capability: &Capability) -> Vec<String> {
    let hints = diff::RenameHints::new();
    let diff = diff::Schema::from(from, to, &hints);
    MigrationStatement::from_diff(&diff, capability)
        .iter()
        .map(|statement| {
            let serializer = match capability.sql {
                Some(toasty_core::driver::Dialect::Sqlite) => {
                    Serializer::sqlite(statement.schema())
                }
                Some(toasty_core::driver::Dialect::Postgresql) => {
                    Serializer::postgresql(statement.schema())
                }
                _ => Serializer::mysql(statement.schema()),
            };
            serializer.serialize(statement.statement())
        })
        .collect()
}

#[test]
fn create_partial_unique_index_sqlite() {
    let sql = migrate(
        &Schema::default(),
        &schema(Some("revoked_at IS NULL")),
        &Capability::SQLITE,
    );

    assert_eq!(sql.len(), 2);
    assert_eq!(
        sql[1],
        "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\") WHERE revoked_at IS NULL;"
    );
}

#[test]
fn create_partial_unique_index_postgresql() {
    let sql = migrate(
        &Schema::default(),
        &schema(Some("revoked_at IS NULL")),
        &Capability::POSTGRESQL,
    );

    assert_eq!(sql.len(), 2);
    assert_eq!(
        sql[1],
        "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\") WHERE revoked_at IS NULL;"
    );
}

#[test]
fn an_index_without_a_filter_has_no_where() {
    let sql = migrate(&Schema::default(), &schema(None), &Capability::SQLITE);

    assert_eq!(
        sql[1],
        "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\");"
    );
}

#[test]
fn adding_a_filter_drops_and_recreates_the_index() {
    let sql = migrate(
        &schema(None),
        &schema(Some("revoked_at IS NULL")),
        &Capability::POSTGRESQL,
    );

    assert_eq!(
        sql,
        [
            "DROP INDEX \"index_tokens_by_owner_and_name\";",
            "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\") WHERE revoked_at IS NULL;",
        ]
    );
}

#[test]
fn changing_the_filter_drops_and_recreates_the_index() {
    let sql = migrate(
        &schema(Some("revoked_at IS NULL")),
        &schema(Some("revoked_at IS NULL AND owner > 0")),
        &Capability::SQLITE,
    );

    assert_eq!(
        sql,
        [
            "DROP INDEX \"index_tokens_by_owner_and_name\";",
            "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\") WHERE revoked_at IS NULL AND owner > 0;",
        ]
    );
}

#[test]
fn removing_the_filter_drops_and_recreates_the_index() {
    let sql = migrate(
        &schema(Some("revoked_at IS NULL")),
        &schema(None),
        &Capability::SQLITE,
    );

    assert_eq!(
        sql,
        [
            "DROP INDEX \"index_tokens_by_owner_and_name\";",
            "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\");",
        ]
    );
}

#[test]
fn an_unchanged_filter_is_no_change() {
    let sql = migrate(
        &schema(Some("revoked_at IS NULL")),
        &schema(Some("revoked_at IS NULL")),
        &Capability::SQLITE,
    );

    assert!(sql.is_empty(), "{sql:?}");
}

#[test]
fn a_rebuild_recreates_the_index_with_its_filter() {
    // Making `name` nullable rebuilds the table on SQLite.
    let mut to = schema(Some("revoked_at IS NULL"));
    to.tables[0].columns[2].nullable = true;

    let sql = migrate(
        &schema(Some("revoked_at IS NULL")),
        &to,
        &Capability::SQLITE,
    );

    assert!(
        sql.iter().any(|s| s.contains("\"_toasty_new_tokens\"")),
        "the table is rebuilt: {sql:#?}"
    );
    assert_eq!(
        sql.iter()
            .filter(|s| s.contains("INDEX"))
            .collect::<Vec<_>>(),
        [
            "CREATE UNIQUE INDEX \"index_tokens_by_owner_and_name\" ON \"tokens\" (\"owner\", \"name\") WHERE revoked_at IS NULL;"
        ]
    );
}

#[test]
#[should_panic(expected = "MySQL does not support partial indexes")]
fn mysql_refuses_to_serialize_a_partial_index() {
    migrate(
        &Schema::default(),
        &schema(Some("revoked_at IS NULL")),
        &Capability::MYSQL,
    );
}
