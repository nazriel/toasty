use super::test_schema;
use crate::engine::simplify::Simplify;
use toasty_core::stmt::{
    self, Expr, ExprColumn, ExprInSubquery, Query, Returning, Select, Source, SourceTable,
    SourceTableId, TableDerived, TableFactor, TableRef, TableWithJoins, Value, Values,
};

/// `SELECT t.column1 FROM (VALUES <rows>) AS t`.
fn select_from_values(rows: Vec<Value>) -> Box<Query> {
    let values = Values::new(
        rows.into_iter()
            .map(|value| Expr::record([Expr::Value(value)]))
            .collect(),
    );
    let select = Select {
        returning: Returning::Project(Expr::record([Expr::column(ExprColumn {
            nesting: 0,
            table: 0,
            column: 0,
        })])),
        source: Source::Table(SourceTable {
            tables: vec![TableRef::Derived(TableDerived {
                subquery: Box::new(Query::values(values)),
            })],
            from: vec![TableWithJoins {
                relation: TableFactor::Table(SourceTableId(0)),
                joins: vec![],
            }],
        }),
        filter: stmt::Filter::ALL,
        distinct: false,
        group_by: vec![],
        having: None,
    };
    Box::new(Query::builder(select).build())
}

fn in_subquery(query: Box<Query>, negated: bool) -> ExprInSubquery {
    ExprInSubquery {
        expr: Box::new(Expr::arg(0)),
        query,
        negated,
    }
}

#[test]
fn in_empty_query_becomes_false() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::SQLITE);

    // `x in (empty_query)` → `false`
    let expr = in_subquery(Box::new(Query::values(Values::default())), false);
    let result = simplify.simplify_expr_in_subquery(&expr);

    assert!(result.unwrap().is_false());
}

#[test]
fn not_in_empty_query_becomes_true() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::SQLITE);

    // `x not in (empty_query)` → `true`
    let expr = in_subquery(Box::new(Query::values(Values::default())), true);
    let result = simplify.simplify_expr_in_subquery(&expr);

    assert!(result.unwrap().is_true());
}

#[test]
fn in_select_from_empty_values_becomes_false() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::SQLITE);

    // `x in (select t.column1 from (values) as t)` → `false`
    let expr = in_subquery(select_from_values(vec![]), false);
    let result = simplify.simplify_expr_in_subquery(&expr);

    assert!(result.unwrap().is_false());
}

#[test]
fn in_select_from_values_of_nulls_becomes_null() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::SQLITE);

    // `x in (select t.column1 from (values (null), (null)) as t)` → `null`
    let expr = in_subquery(select_from_values(vec![Value::Null, Value::Null]), false);
    let result = simplify.simplify_expr_in_subquery(&expr);

    assert!(result.unwrap().is_value_null());
}

#[test]
fn in_select_from_values_with_a_value_not_simplified() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::SQLITE);

    // `x in (select t.column1 from (values (null), (1)) as t)`, may match
    let expr = in_subquery(
        select_from_values(vec![Value::Null, Value::from(1i64)]),
        false,
    );
    let result = simplify.simplify_expr_in_subquery(&expr);

    assert!(result.is_none());
}
