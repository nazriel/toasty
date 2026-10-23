//! Verifies that a table or an enum type with a schema is named
//! `"schema"."name"` everywhere PostgreSQL SQL refers to it: in `SELECT`,
//! `INSERT`, `UPDATE` and `DELETE`, in column references qualified by their
//! table, and in the DDL the migration generator emits. An index is created in
//! its table's schema, so `CREATE INDEX` names it unqualified and `DROP INDEX`
//! qualifies it; `ALTER ... RENAME TO` names the new name unqualified.
//!
//! Tests construct the AST and schemas directly so the serializer and the
//! migration generator are exercised in isolation.

use expect_test::expect;
use toasty_core::{
    driver::Capability,
    schema::{
        db::{
            Column, ColumnId, EnumVariant, Index, IndexColumn, IndexId, IndexOp, IndexScope,
            PrimaryKey, Schema, Table, TableId, Type as StorageType, TypeEnum,
        },
        diff,
    },
    stmt::{
        self, Assignments, Delete, Expr, ExprColumn, Filter, Insert, InsertTable, InsertTarget,
        Join, JoinOp, Returning, Select, Source, SourceTable, SourceTableId, TableFactor, TableRef,
        TableWithJoins, Update, UpdateTarget, Values,
    },
};
use toasty_sql::{Serializer, Statement as SqlStatement, migration::MigrationStatement};

fn make_column(table_id: usize, index: usize, name: &str, storage_ty: StorageType) -> Column {
    Column {
        id: ColumnId {
            table: TableId(table_id),
            index,
        },
        name: name.to_string(),
        ty: match storage_ty {
            StorageType::Integer(_) => stmt::Type::I64,
            _ => stmt::Type::String,
        },
        storage_ty,
        nullable: false,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    }
}

fn make_index(table_id: usize, index: usize, name: &str, column: usize) -> Index {
    Index {
        id: IndexId {
            table: TableId(table_id),
            index,
        },
        name: name.to_string(),
        on: TableId(table_id),
        columns: vec![IndexColumn {
            column: ColumnId {
                table: TableId(table_id),
                index: column,
            },
            op: IndexOp::Eq,
            scope: IndexScope::Local,
        }],
        unique: false,
        primary_key: false,
        filter: None,
    }
}

/// A table in `schema` whose first column, `id`, is the primary key, with the
/// primary key's index first in `indices`, as the schema builder lays them out.
fn make_table(
    id: usize,
    schema: Option<&str>,
    name: &str,
    columns: Vec<Column>,
    indices: Vec<Index>,
) -> Table {
    let pk_index = Index {
        primary_key: true,
        unique: true,
        ..make_index(id, 0, &format!("index_{name}_by_id"), 0)
    };

    Table {
        id: TableId(id),
        schema: schema.map(str::to_string),
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

fn make_enum_type(schema: Option<&str>, name: &str, variants: &[&str]) -> TypeEnum {
    TypeEnum {
        schema: schema.map(str::to_string),
        name: Some(name.to_string()),
        variants: variants
            .iter()
            .map(|v| EnumVariant {
                name: v.to_string(),
            })
            .collect(),
    }
}

/// `"inventory"."items" (id, name)` and `"audit"."entries" (id, item_id)`.
fn two_schemas() -> Schema {
    Schema {
        tables: vec![
            make_table(
                0,
                Some("inventory"),
                "items",
                vec![
                    make_column(0, 0, "id", StorageType::Integer(8)),
                    make_column(0, 1, "name", StorageType::Text),
                ],
                vec![],
            ),
            make_table(
                1,
                Some("audit"),
                "entries",
                vec![
                    make_column(1, 0, "id", StorageType::Integer(8)),
                    make_column(1, 1, "item_id", StorageType::Integer(8)),
                ],
                vec![],
            ),
        ],
    }
}

/// Reference to column `column` of the `table`th entry in `SourceTable::tables`.
fn col(table: usize, column: usize) -> Expr {
    Expr::column(ExprColumn {
        nesting: 0,
        table,
        column,
    })
}

fn render_postgresql(schema: &Schema, stmt: stmt::Statement) -> String {
    Serializer::postgresql(schema).serialize(&SqlStatement::from(stmt))
}

fn serialize_migration(stmts: &[MigrationStatement<'_>]) -> Vec<String> {
    stmts
        .iter()
        .map(|ms| Serializer::postgresql(ms.schema()).serialize(ms.statement()))
        .collect()
}

fn migrate(from: &Schema, to: &Schema, hints: &diff::RenameHints) -> Vec<String> {
    let diff = diff::Schema::from(from, to, hints);
    serialize_migration(&MigrationStatement::from_diff(
        &diff,
        &Capability::POSTGRESQL,
    ))
}

// -----------------------------------------------------------------------------
// DML
// -----------------------------------------------------------------------------

#[test]
fn select_from_a_schema_qualified_table() {
    let select = Select {
        returning: Returning::Project(Expr::record([col(0, 1)])),
        source: Source::from(TableId(0)),
        filter: Filter::new(Expr::eq(col(0, 0), Expr::from(1i64))),
        distinct: false,
        group_by: vec![],
        having: None,
    };
    let stmt = stmt::Statement::Query(stmt::Query::builder(select).build());

    expect![[r#"SELECT tbl_0_0."name" AS column1 FROM "inventory"."items" AS tbl_0_0 WHERE tbl_0_0."id" = 1;"#]]
        .assert_eq(&render_postgresql(&two_schemas(), stmt));
}

#[test]
fn join_between_two_schemas() {
    let source = Source::Table(SourceTable {
        tables: vec![TableRef::Table(TableId(0)), TableRef::Table(TableId(1))],
        from: vec![TableWithJoins {
            relation: TableFactor::Table(SourceTableId(0)),
            joins: vec![Join {
                table: SourceTableId(1),
                constraint: JoinOp::Left(Expr::eq(col(1, 1), col(0, 0))),
            }],
        }],
    });
    let select = Select {
        returning: Returning::Project(Expr::record([col(0, 1), col(1, 0)])),
        source,
        filter: Filter::ALL,
        distinct: false,
        group_by: vec![],
        having: None,
    };
    let stmt = stmt::Statement::Query(stmt::Query::builder(select).build());

    expect![[r#"SELECT tbl_0_0."name" AS column1, tbl_0_1."id" AS column2 FROM "inventory"."items" AS tbl_0_0 LEFT JOIN "audit"."entries" AS tbl_0_1 ON tbl_0_1."item_id" = tbl_0_0."id";"#]]
        .assert_eq(&render_postgresql(&two_schemas(), stmt));
}

fn insert_item(upsert: Option<Box<stmt::Upsert>>) -> stmt::Statement {
    Insert {
        target: InsertTarget::Table(InsertTable {
            table: TableId(0),
            columns: vec![
                ColumnId {
                    table: TableId(0),
                    index: 0,
                },
                ColumnId {
                    table: TableId(0),
                    index: 1,
                },
            ],
        }),
        source: stmt::Query::values(Values::new(vec![Expr::record([
            Expr::from(1i64),
            Expr::from("a"),
        ])])),
        upsert,
        returning: Some(Returning::Project(Expr::record([col(0, 0)]))),
    }
    .into()
}

#[test]
fn insert_into_a_schema_qualified_table() {
    expect![[r#"INSERT INTO "inventory"."items" ("id", "name") VALUES (1, 'a') RETURNING "id" AS column1;"#]]
        .assert_eq(&render_postgresql(&two_schemas(), insert_item(None)));
}

/// The conflict action names the existing row's columns through the table,
/// which PostgreSQL accepts schema-qualified.
#[test]
fn upsert_into_a_schema_qualified_table() {
    let mut shared = Assignments::default();
    shared.set(1usize, col(0, 1));
    let upsert = stmt::Upsert {
        target: stmt::UpsertTarget::Columns(vec![ColumnId {
            table: TableId(0),
            index: 0,
        }]),
        shared,
        defaults: Assignments::default(),
        update_defaults: Assignments::default(),
        create: Assignments::default(),
        update: Assignments::default(),
        action: stmt::UpsertAction::Update,
    };

    expect![[r#"INSERT INTO "inventory"."items" ("id", "name") VALUES (1, 'a') ON CONFLICT ("id") DO UPDATE SET "name" = "inventory"."items"."name" RETURNING "id" AS column1;"#]]
        .assert_eq(&render_postgresql(
            &two_schemas(),
            insert_item(Some(Box::new(upsert))),
        ));
}

#[test]
fn update_a_schema_qualified_table() {
    let mut assignments = Assignments::default();
    assignments.set(1usize, Expr::from("b"));
    let stmt = Update {
        target: UpdateTarget::Table(TableId(0)),
        assignments,
        filter: Filter::new(Expr::eq(col(0, 0), Expr::from(1i64))),
        condition: stmt::Condition::default(),
        returning: None,
    }
    .into();

    expect![[r#"UPDATE "inventory"."items" AS tbl_0_0 SET "name" = 'b' WHERE "id" = 1;"#]]
        .assert_eq(&render_postgresql(&two_schemas(), stmt));
}

#[test]
fn delete_from_a_schema_qualified_table() {
    let stmt = Delete {
        from: Source::from(TableId(1)),
        filter: Filter::new(Expr::eq(col(0, 1), Expr::from(1i64))),
        returning: None,
        condition: stmt::Condition::default(),
    }
    .into();

    expect![[r#"DELETE FROM "audit"."entries" AS tbl_0_0 WHERE tbl_0_0."item_id" = 1;"#]]
        .assert_eq(&render_postgresql(&two_schemas(), stmt));
}

// -----------------------------------------------------------------------------
// DDL from the migration generator
// -----------------------------------------------------------------------------

/// `"inventory"."items" (id, sku, status "inventory"."item_status")` with an
/// index on `sku`.
fn items_schema(status: TypeEnum) -> Schema {
    Schema {
        tables: vec![make_table(
            0,
            Some("inventory"),
            "items",
            vec![
                make_column(0, 0, "id", StorageType::Integer(8)),
                make_column(0, 1, "sku", StorageType::Text),
                make_column(0, 2, "status", StorageType::Enum(status)),
            ],
            vec![make_index(0, 1, "index_items_by_sku", 1)],
        )],
    }
}

fn item_status(variants: &[&str]) -> TypeEnum {
    make_enum_type(Some("inventory"), "item_status", variants)
}

#[test]
fn create_type_table_and_index_in_a_schema() {
    let to = items_schema(item_status(&["new", "sold"]));

    let sql = migrate(&Schema::default(), &to, &diff::RenameHints::new());

    expect![[r#"
        [
            "CREATE TYPE \"inventory\".\"item_status\" AS ENUM ('new', 'sold');",
            "CREATE TABLE \"inventory\".\"items\" (\n    \"id\" BIGINT NOT NULL,\n    \"sku\" TEXT NOT NULL,\n    \"status\" \"inventory\".\"item_status\" NOT NULL,\n    PRIMARY KEY (\"id\")\n);",
            "CREATE INDEX \"index_items_by_sku\" ON \"inventory\".\"items\" (\"sku\");",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
fn drop_index_names_the_tables_schema() {
    let from = items_schema(item_status(&["new"]));
    let mut to = from.clone();
    to.tables[0].indices.truncate(1);

    let sql = migrate(&from, &to, &diff::RenameHints::new());

    expect![[r#"
        [
            "DROP INDEX \"inventory\".\"index_items_by_sku\";",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
fn drop_a_schema_qualified_table() {
    let from = items_schema(item_status(&["new"]));

    let sql = migrate(&from, &Schema::default(), &diff::RenameHints::new());

    expect![[r#"
        [
            "DROP TABLE \"inventory\".\"items\";",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
fn rename_table_keeps_the_new_name_unqualified() {
    let from = items_schema(item_status(&["new"]));
    let mut to = from.clone();
    to.tables[0].name = "products".to_string();
    let mut hints = diff::RenameHints::new();
    hints.add_table_hint(TableId(0), TableId(0));

    let sql = migrate(&from, &to, &hints);

    expect![[r#"
        [
            "ALTER TABLE \"inventory\".\"items\" RENAME TO \"products\";",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
fn add_enum_variant_to_a_schema_qualified_type() {
    let from = items_schema(item_status(&["new"]));
    let to = items_schema(item_status(&["new", "sold"]));

    let sql = migrate(&from, &to, &diff::RenameHints::new());

    expect![[r#"
        [
            "ALTER TYPE \"inventory\".\"item_status\" ADD VALUE 'sold';",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
fn rename_a_schema_qualified_enum_type() {
    let from = items_schema(item_status(&["new"]));
    let to = items_schema(make_enum_type(Some("inventory"), "stock_status", &["new"]));

    let sql = migrate(&from, &to, &diff::RenameHints::new());

    expect![[r#"
        [
            "ALTER TYPE \"inventory\".\"item_status\" RENAME TO \"stock_status\";",
        ]
    "#]]
    .assert_debug_eq(&sql);
}

#[test]
#[should_panic(
    expected = "cannot move the table \"items\" from schema \"inventory\" to schema \"audit\""
)]
fn moving_a_table_to_another_schema_panics() {
    let from = items_schema(item_status(&["new"]));
    let mut to = from.clone();
    to.tables[0].schema = Some("audit".to_string());
    let mut hints = diff::RenameHints::new();
    hints.add_table_hint(TableId(0), TableId(0));

    migrate(&from, &to, &hints);
}

#[test]
#[should_panic(
    expected = "cannot move the enum type \"item_status\" from schema \"inventory\" to the default schema"
)]
fn moving_an_enum_type_to_another_schema_panics() {
    let from = items_schema(item_status(&["new"]));
    let to = items_schema(make_enum_type(None, "item_status", &["new"]));

    migrate(&from, &to, &diff::RenameHints::new());
}
