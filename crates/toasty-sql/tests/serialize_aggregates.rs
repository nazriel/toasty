//! Verifies the serializer renders aggregate functions, `GROUP BY` and
//! `HAVING`, and the casts that make each database return an aggregate in
//! the type the query decodes.

use expect_test::expect;
use toasty_core::{
    schema::db::{Column, ColumnId, PrimaryKey, Schema, Table, TableId, Type as StorageType},
    stmt::{
        self, AggregateFunc, Direction, Expr, ExprColumn, Filter, OrderBy, OrderByExpr, Returning,
        Select, Source, SourceTable, SourceTableId, TableFactor, TableRef, TableWithJoins,
    },
};
use toasty_sql::{Serializer, Statement as SqlStatement};

fn column(index: usize, name: &str, ty: stmt::Type, storage_ty: StorageType) -> Column {
    Column {
        id: ColumnId {
            table: TableId(0),
            index,
        },
        name: name.to_string(),
        ty,
        storage_ty,
        nullable: index != 0,
        primary_key: index == 0,
        auto_increment: false,
        versionable: false,
    }
}

/// `orders(id BIGINT, status TEXT, total INTEGER, weight DOUBLE)`.
fn orders_schema() -> Schema {
    Schema {
        tables: vec![Table {
            id: TableId(0),
            name: "orders".to_string(),
            columns: vec![
                column(0, "id", stmt::Type::I64, StorageType::Integer(8)),
                column(1, "status", stmt::Type::String, StorageType::Text),
                column(2, "total", stmt::Type::I32, StorageType::Integer(4)),
                column(3, "weight", stmt::Type::F64, StorageType::Float(8)),
            ],
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

fn col(column: usize) -> Expr {
    Expr::column(ExprColumn {
        nesting: 0,
        table: 0,
        column,
    })
}

#[derive(Clone, Copy)]
enum Flavor {
    Sqlite,
    Postgresql,
    Mysql,
}

fn render(flavor: Flavor, stmt: stmt::Statement) -> String {
    let schema = orders_schema();
    let sql_stmt = SqlStatement::from(stmt);
    let serializer = match flavor {
        Flavor::Sqlite => Serializer::sqlite(&schema),
        Flavor::Postgresql => Serializer::postgresql(&schema),
        Flavor::Mysql => Serializer::mysql(&schema),
    };
    serializer.serialize(&sql_stmt)
}

/// `SELECT status, COUNT(*), COUNT(weight), SUM(total), AVG(total),
/// MIN(total), MAX(weight) FROM orders GROUP BY status HAVING SUM(total) >
/// 10 ORDER BY COUNT(*) DESC`.
fn grouped_query() -> stmt::Statement {
    let select = Select {
        returning: Returning::Project(Expr::record([
            col(1),
            Expr::count_star(),
            Expr::count(col(3)),
            Expr::aggregate(AggregateFunc::Sum, col(2)),
            Expr::aggregate(AggregateFunc::Avg, col(2)),
            Expr::aggregate(AggregateFunc::Min, col(2)),
            Expr::aggregate(AggregateFunc::Max, col(3)),
        ])),
        source: Source::Table(SourceTable {
            tables: vec![TableRef::Table(TableId(0))],
            from: vec![TableWithJoins {
                relation: TableFactor::Table(SourceTableId(0)),
                joins: vec![],
            }],
        }),
        filter: Filter::ALL,
        distinct: false,
        group_by: vec![col(1)],
        having: Some(Expr::gt(
            Expr::aggregate(AggregateFunc::Sum, col(2)),
            Expr::from(10i64),
        )),
    };
    let mut query = stmt::Query::builder(select).build();
    query.order_by = Some(OrderBy::from(OrderByExpr {
        expr: Expr::count_star(),
        order: Some(Direction::Desc),
        nulls: None,
    }));
    stmt::Statement::Query(query)
}

#[test]
fn grouped_aggregates_sqlite() {
    expect![[r#"SELECT tbl_0_0."status" AS column1, COUNT(*) AS column2, COUNT(tbl_0_0."weight") AS column3, SUM(tbl_0_0."total") AS column4, AVG(tbl_0_0."total") AS column5, MIN(tbl_0_0."total") AS column6, MAX(tbl_0_0."weight") AS column7 FROM "orders" AS tbl_0_0 GROUP BY tbl_0_0."status" HAVING SUM(tbl_0_0."total") > 10 ORDER BY COUNT(*) DESC;"#]]
        .assert_eq(&render(Flavor::Sqlite, grouped_query()));
}

#[test]
fn grouped_aggregates_postgresql_cast_sum_and_average() {
    expect![[r#"SELECT tbl_0_0."status" AS column1, COUNT(*) AS column2, COUNT(tbl_0_0."weight") AS column3, CAST(SUM(tbl_0_0."total") AS INTEGER) AS column4, CAST(AVG(tbl_0_0."total") AS DOUBLE PRECISION) AS column5, MIN(tbl_0_0."total") AS column6, MAX(tbl_0_0."weight") AS column7 FROM "orders" AS tbl_0_0 GROUP BY tbl_0_0."status" HAVING CAST(SUM(tbl_0_0."total") AS INTEGER) > 10 ORDER BY COUNT(*) DESC;"#]]
        .assert_eq(&render(Flavor::Postgresql, grouped_query()));
}

#[test]
fn grouped_aggregates_mysql_cast_sum_and_average() {
    expect![[r#"SELECT tbl_0_0.`status` AS column_0, COUNT(*) AS column_1, COUNT(tbl_0_0.`weight`) AS column_2, CAST(SUM(tbl_0_0.`total`) AS SIGNED) AS column_3, CAST(AVG(tbl_0_0.`total`) AS DOUBLE) AS column_4, MIN(tbl_0_0.`total`) AS column_5, MAX(tbl_0_0.`weight`) AS column_6 FROM `orders` AS tbl_0_0 GROUP BY tbl_0_0.`status` HAVING CAST(SUM(tbl_0_0.`total`) AS SIGNED) > 10 ORDER BY COUNT(*) DESC;"#]]
        .assert_eq(&render(Flavor::Mysql, grouped_query()));
}

#[test]
fn sum_of_a_float_column_is_not_cast() {
    let sum_weight = || {
        let select = Select {
            returning: Returning::Project(Expr::record([Expr::aggregate(
                AggregateFunc::Sum,
                col(3),
            )])),
            source: Source::table(TableId(0)),
            filter: Filter::ALL,
            distinct: false,
            group_by: vec![],
            having: None,
        };
        stmt::Statement::Query(stmt::Query::builder(select).build())
    };
    expect![[r#"SELECT SUM(tbl_0_0."weight") AS column1 FROM "orders" AS tbl_0_0;"#]]
        .assert_eq(&render(Flavor::Postgresql, sum_weight()));
    expect!["SELECT SUM(tbl_0_0.`weight`) AS column_0 FROM `orders` AS tbl_0_0;"]
        .assert_eq(&render(Flavor::Mysql, sum_weight()));
}
