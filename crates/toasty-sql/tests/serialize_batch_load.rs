//! Verifies the serializer renders the filters the planner emits to batch
//! load related rows for a set of parent keys:
//!
//! * SQLite: `fk IN (SELECT column1 FROM (VALUES ...))`
//! * PostgreSQL: `fk = ANY($1)`, the keys bound as one array parameter
//! * MySQL: a correlated `EXISTS` over the `VALUES` table
//!
//! Each test constructs the AST directly, in the shape the planner produces
//! once the keys are substituted, so the serializer is exercised in
//! isolation — no lowering pipeline involved.

use expect_test::expect;
use toasty_core::{
    schema::db::{Column, ColumnId, PrimaryKey, Schema, Table, TableId, Type as StorageType},
    stmt::{
        self, BinaryOp, Expr, ExprColumn, Filter, Query, Returning, Select, Source, SourceTable,
        SourceTableId, TableDerived, TableFactor, TableRef, TableWithJoins, Values,
    },
};
use toasty_sql::{Serializer, Statement as SqlStatement};

/// `children (id INTEGER PRIMARY KEY, parent_id INTEGER)`.
fn children_schema() -> Schema {
    let column = |index: usize, name: &str| Column {
        id: ColumnId {
            table: TableId(0),
            index,
        },
        name: name.to_string(),
        ty: stmt::Type::I64,
        storage_ty: StorageType::Integer(8),
        nullable: false,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    };

    Schema {
        tables: vec![Table {
            id: TableId(0),
            schema: None,
            name: "children".to_string(),
            columns: vec![column(0, "id"), column(1, "parent_id")],
            primary_key: PrimaryKey {
                columns: vec![ColumnId {
                    table: TableId(0),
                    index: 0,
                }],
                index: toasty_core::schema::db::IndexId {
                    table: TableId(0),
                    index: 0,
                },
            },
            indices: vec![],
        }],
    }
}

/// Reference to column `column` of the `table`th entry of the source,
/// `nesting` queries out.
fn col(nesting: usize, table: usize, column: usize) -> Expr {
    Expr::column(ExprColumn {
        nesting,
        table,
        column,
    })
}

/// A source reading the single table `table_ref`.
fn source(table_ref: TableRef) -> Source {
    Source::Table(SourceTable {
        tables: vec![table_ref],
        from: vec![TableWithJoins {
            relation: TableFactor::Table(SourceTableId(0)),
            joins: vec![],
        }],
    })
}

fn select(returning: Expr, source: Source, filter: Filter) -> Select {
    Select {
        returning: Returning::Project(returning),
        source,
        filter,
        distinct: false,
        group_by: vec![],
        having: None,
    }
}

/// `(VALUES (<arg 0>), (<arg 1>))`, two parent keys.
fn keys() -> TableRef {
    TableRef::Derived(TableDerived {
        subquery: Box::new(Query::values(Values::new(vec![
            Expr::record([Expr::arg(0)]),
            Expr::record([Expr::arg(1)]),
        ]))),
    })
}

/// `SELECT id, parent_id FROM children WHERE <filter>`.
fn load_children(filter: Expr) -> stmt::Statement {
    let returning = Expr::record([col(0, 0, 0), col(0, 0, 1)]);
    let select = select(
        returning,
        source(TableRef::Table(TableId(0))),
        Filter::new(filter),
    );
    stmt::Statement::Query(Query::builder(select).build())
}

#[test]
fn batch_load_in_values_sqlite() {
    let schema = children_schema();
    let key_set = select(Expr::record([col(0, 0, 0)]), source(keys()), Filter::ALL);
    let filter = Expr::in_subquery(col(0, 0, 1), key_set);

    expect![[r#"SELECT tbl_0_0."id" AS column1, tbl_0_0."parent_id" AS column2 FROM "children" AS tbl_0_0 WHERE tbl_0_0."parent_id" IN (SELECT tbl_1_0.column1 AS column1 FROM (VALUES (?1), (?2)) AS tbl_1_0);"#]]
        .assert_eq(&Serializer::sqlite(&schema).serialize(&SqlStatement::from(load_children(filter))));
}

#[test]
fn batch_load_any_array_postgresql() {
    let schema = children_schema();
    let filter = Expr::any_op(col(0, 0, 1), BinaryOp::Eq, Expr::arg(0));

    expect![[r#"SELECT tbl_0_0."id" AS column1, tbl_0_0."parent_id" AS column2 FROM "children" AS tbl_0_0 WHERE tbl_0_0."parent_id" = ANY($1);"#]]
        .assert_eq(&Serializer::postgresql(&schema).serialize(&SqlStatement::from(load_children(filter))));
}

#[test]
fn batch_load_exists_values_mysql() {
    let schema = children_schema();
    let correlated = select(
        Expr::record([Expr::from(1i64)]),
        source(keys()),
        Filter::new(Expr::eq(col(0, 0, 0), col(1, 0, 1))),
    );
    let filter = Expr::exists(correlated);

    expect!["SELECT tbl_0_0.`id` AS column_0, tbl_0_0.`parent_id` AS column_1 FROM `children` AS tbl_0_0 WHERE EXISTS (SELECT 1 AS column_0 FROM (VALUES ROW(?), ROW(?)) AS tbl_1_0 WHERE tbl_1_0.column_0 = tbl_0_0.`parent_id`);"]
        .assert_eq(&Serializer::mysql(&schema).serialize(&SqlStatement::from(load_children(filter))));
}
