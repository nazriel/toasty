//! Verifies the serializer renders `SELECT` shape — WHERE, ORDER BY,
//! LIMIT/OFFSET, and `FOR UPDATE` / `FOR SHARE` row-level locks.
//!
//! Joins and ANY/ALL operators have their own files; this file focuses on the
//! clauses around the `SELECT` body. Each test constructs the AST directly so
//! the serializer is exercised in isolation — no lowering pipeline involved.

use expect_test::expect;
use toasty_core::{
    schema::db::{Column, ColumnId, PrimaryKey, Schema, Table, TableId, Type as StorageType},
    stmt::{
        self, Direction, Expr, ExprColumn, Filter, Limit, LimitOffset, Lock, OrderBy, OrderByExpr,
        Returning, Select, Source, SourceTable, SourceTableId, TableFactor, TableRef,
        TableWithJoins,
    },
};
use toasty_sql::{Serializer, Statement as SqlStatement};

/// Minimal `id INTEGER PRIMARY KEY, *cols` table. `cols` start at column index
/// 1 and are all `INTEGER`.
fn make_table(id: usize, name: &str, cols: &[&str]) -> Table {
    let mut columns = vec![Column {
        id: ColumnId {
            table: TableId(id),
            index: 0,
        },
        name: "id".to_string(),
        ty: stmt::Type::I64,
        storage_ty: StorageType::Integer(8),
        nullable: false,
        primary_key: true,
        auto_increment: false,
        versionable: false,
    }];
    for (i, name) in cols.iter().enumerate() {
        columns.push(Column {
            id: ColumnId {
                table: TableId(id),
                index: i + 1,
            },
            name: (*name).to_string(),
            ty: stmt::Type::I64,
            storage_ty: StorageType::Integer(8),
            nullable: false,
            primary_key: false,
            auto_increment: false,
            versionable: false,
        });
    }
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
            index: toasty_core::schema::db::IndexId {
                table: TableId(id),
                index: 0,
            },
        },
        indices: vec![],
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

/// SQL flavor selector for the `render` helper.
#[derive(Clone, Copy)]
enum Flavor {
    Sqlite,
    Postgresql,
    Mysql,
}

fn render(flavor: Flavor, schema: &Schema, stmt: stmt::Statement) -> String {
    let sql_stmt = SqlStatement::from(stmt);
    let serializer = match flavor {
        Flavor::Sqlite => Serializer::sqlite(schema),
        Flavor::Postgresql => Serializer::postgresql(schema),
        Flavor::Mysql => Serializer::mysql(schema),
    };
    serializer.serialize(&sql_stmt)
}

/// Schema with a single `users(id, name)` table.
fn users_schema() -> Schema {
    Schema {
        tables: vec![make_table(0, "users", &["name"])],
    }
}

/// Single-table `users` source.
fn users_source() -> Source {
    Source::Table(SourceTable {
        tables: vec![TableRef::Table(TableId(0))],
        from: vec![TableWithJoins {
            relation: TableFactor::Table(SourceTableId(0)),
            joins: vec![],
        }],
    })
}

/// `SELECT id FROM users` with the given filter, ordering, limit, and locks.
fn make_query(
    filter: Filter,
    order_by: Option<OrderBy>,
    limit: Option<Limit>,
    locks: Vec<Lock>,
) -> stmt::Statement {
    let select = Select {
        returning: Returning::Project(Expr::record([col(0, 0)])),
        source: users_source(),
        filter,
        distinct: false,
        group_by: vec![],
        having: None,
    };
    let mut query = stmt::Query::builder(select).build();
    query.order_by = order_by;
    query.limit = limit;
    query.locks = locks;
    stmt::Statement::Query(query)
}

#[test]
fn select_basic() {
    let schema = users_schema();
    let stmt = make_query(Filter::ALL, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0;"#]].assert_eq(&render(
        Flavor::Sqlite,
        &schema,
        stmt,
    ));
    let stmt = make_query(Filter::ALL, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0;"#]].assert_eq(&render(
        Flavor::Postgresql,
        &schema,
        stmt,
    ));
    let stmt = make_query(Filter::ALL, None, None, vec![]);
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0;"].assert_eq(&render(
        Flavor::Mysql,
        &schema,
        stmt,
    ));
}

#[test]
fn select_distinct() {
    let schema = users_schema();
    // `make_query` hard-codes `distinct: false`, so build the `Select`
    // directly. `render` consumes the statement, so rebuild it per flavor.
    let distinct_query = || {
        let select = Select {
            returning: Returning::Project(Expr::record([col(0, 0)])),
            source: users_source(),
            filter: Filter::ALL,
            distinct: true,
            group_by: vec![],
            having: None,
        };
        stmt::Statement::Query(stmt::Query::builder(select).build())
    };
    expect![[r#"SELECT DISTINCT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, distinct_query()));
    expect![[r#"SELECT DISTINCT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0;"#]]
        .assert_eq(&render(Flavor::Postgresql, &schema, distinct_query()));
    expect!["SELECT DISTINCT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0;"]
        .assert_eq(&render(Flavor::Mysql, &schema, distinct_query()));
}

#[test]
fn select_with_where() {
    let schema = users_schema();
    let filter = Filter::from(Expr::eq(col(0, 0), Expr::from(1i64)));
    let stmt = make_query(filter, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 WHERE tbl_0_0."id" = 1;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
    let filter = Filter::from(Expr::eq(col(0, 0), Expr::from(1i64)));
    let stmt = make_query(filter, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 WHERE tbl_0_0."id" = 1;"#]]
        .assert_eq(&render(Flavor::Postgresql, &schema, stmt));
    let filter = Filter::from(Expr::eq(col(0, 0), Expr::from(1i64)));
    let stmt = make_query(filter, None, None, vec![]);
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0 WHERE tbl_0_0.`id` = 1;"]
        .assert_eq(&render(Flavor::Mysql, &schema, stmt));
}

#[test]
fn select_with_and_filter() {
    let schema = users_schema();
    let filter = Filter::from(Expr::and(
        Expr::eq(col(0, 0), Expr::from(1i64)),
        Expr::eq(col(0, 1), Expr::from("foo")),
    ));
    let stmt = make_query(filter, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 WHERE tbl_0_0."id" = 1 AND tbl_0_0."name" = 'foo';"#]].assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_or_filter() {
    let schema = users_schema();
    let filter = Filter::from(Expr::or(
        Expr::eq(col(0, 0), Expr::from(1i64)),
        Expr::eq(col(0, 0), Expr::from(2i64)),
    ));
    let stmt = make_query(filter, None, None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 WHERE tbl_0_0."id" = 1 OR tbl_0_0."id" = 2;"#]]
    .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_order_by_asc() {
    let schema = users_schema();
    let order_by = OrderBy {
        exprs: vec![OrderByExpr {
            expr: col(0, 0),
            order: Some(Direction::Asc),
            nulls: None,
        }],
    };
    let stmt = make_query(Filter::ALL, Some(order_by), None, vec![]);
    expect![[
        r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."id" ASC;"#
    ]]
    .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_order_by_desc() {
    let schema = users_schema();
    let order_by = OrderBy {
        exprs: vec![OrderByExpr {
            expr: col(0, 0),
            order: Some(Direction::Desc),
            nulls: None,
        }],
    };
    let stmt = make_query(Filter::ALL, Some(order_by), None, vec![]);
    expect![[
        r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."id" DESC;"#
    ]]
    .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_order_by_multiple_columns() {
    let schema = users_schema();
    let order_by = OrderBy {
        exprs: vec![
            OrderByExpr {
                expr: col(0, 1),
                order: Some(Direction::Asc),
                nulls: None,
            },
            OrderByExpr {
                expr: col(0, 0),
                order: Some(Direction::Desc),
                nulls: None,
            },
        ],
    };
    let stmt = make_query(Filter::ALL, Some(order_by), None, vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."name" ASC, tbl_0_0."id" DESC;"#]].assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

/// `ORDER BY name <direction> NULLS <placement>` for `flavor`.
fn render_nulls(flavor: Flavor, direction: Direction, nulls: stmt::Nulls) -> String {
    let schema = users_schema();
    let order_by = OrderBy {
        exprs: vec![OrderByExpr {
            expr: col(0, 1),
            order: Some(direction),
            nulls: Some(nulls),
        }],
    };
    render(
        flavor,
        &schema,
        make_query(Filter::ALL, Some(order_by), None, vec![]),
    )
}

#[test]
fn select_with_order_by_nulls_last() {
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."name" ASC NULLS LAST;"#]]
        .assert_eq(&render_nulls(Flavor::Sqlite, Direction::Asc, stmt::Nulls::Last));
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."name" DESC NULLS LAST;"#]]
        .assert_eq(&render_nulls(Flavor::Postgresql, Direction::Desc, stmt::Nulls::Last));
    // MySQL has no `NULLS LAST`: a leading `IS NULL` key puts the nulls after
    // the values.
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0 ORDER BY tbl_0_0.`name` IS NULL, tbl_0_0.`name` ASC;"]
        .assert_eq(&render_nulls(Flavor::Mysql, Direction::Asc, stmt::Nulls::Last));
}

#[test]
fn select_with_order_by_nulls_first() {
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."name" DESC NULLS FIRST;"#]]
        .assert_eq(&render_nulls(Flavor::Sqlite, Direction::Desc, stmt::Nulls::First));
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 ORDER BY tbl_0_0."name" ASC NULLS FIRST;"#]]
        .assert_eq(&render_nulls(Flavor::Postgresql, Direction::Asc, stmt::Nulls::First));
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0 ORDER BY tbl_0_0.`name` IS NOT NULL, tbl_0_0.`name` DESC;"]
        .assert_eq(&render_nulls(Flavor::Mysql, Direction::Desc, stmt::Nulls::First));
}

#[test]
fn select_with_limit() {
    let schema = users_schema();
    let limit = Limit::Offset(LimitOffset {
        limit: Expr::from(10i64),
        offset: None,
    });
    let stmt = make_query(Filter::ALL, None, Some(limit), vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 LIMIT 10;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_limit_and_offset() {
    let schema = users_schema();
    let limit = Limit::Offset(LimitOffset {
        limit: Expr::from(10i64),
        offset: Some(Expr::from(20i64)),
    });
    let stmt = make_query(Filter::ALL, None, Some(limit), vec![]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 LIMIT 10 OFFSET 20;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
}

#[test]
fn select_with_for_update() {
    let schema = users_schema();
    // SQLite has no row-level locks, but the serializer renders `FOR UPDATE`
    // unconditionally — verify that for all three flavors.
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Update]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 FOR UPDATE;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Update]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 FOR UPDATE;"#]]
        .assert_eq(&render(Flavor::Postgresql, &schema, stmt));
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Update]);
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0 FOR UPDATE;"]
        .assert_eq(&render(Flavor::Mysql, &schema, stmt));
}

#[test]
fn select_with_for_share() {
    let schema = users_schema();
    // As with `FOR UPDATE`, the serializer renders `FOR SHARE` unconditionally
    // across flavors; database compatibility is enforced at execution, not here.
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Share]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 FOR SHARE;"#]]
        .assert_eq(&render(Flavor::Sqlite, &schema, stmt));
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Share]);
    expect![[r#"SELECT tbl_0_0."id" AS column1 FROM "users" AS tbl_0_0 FOR SHARE;"#]]
        .assert_eq(&render(Flavor::Postgresql, &schema, stmt));
    let stmt = make_query(Filter::ALL, None, None, vec![Lock::Share]);
    expect!["SELECT tbl_0_0.`id` AS column_0 FROM `users` AS tbl_0_0 FOR SHARE;"]
        .assert_eq(&render(Flavor::Mysql, &schema, stmt));
}
