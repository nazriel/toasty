use super::test_schema;
use crate::engine::simplify::Simplify;
use toasty_core::stmt::{BinaryOp, Expr, ExprAnyOp, Value};

fn any_eq(rhs: Vec<Value>) -> ExprAnyOp {
    ExprAnyOp {
        lhs: Box::new(Expr::arg(0)),
        op: BinaryOp::Eq,
        rhs: Box::new(Expr::Value(Value::List(rhs))),
    }
}

#[test]
fn empty_list_becomes_false() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::POSTGRESQL);

    // `x = any([])` → `false`
    let result = simplify.simplify_expr_any_op(&any_eq(vec![]));

    assert!(result.unwrap().is_false());
}

#[test]
fn list_of_nulls_becomes_null() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::POSTGRESQL);

    // `x = any([null, null])` → `null`
    let result = simplify.simplify_expr_any_op(&any_eq(vec![Value::Null, Value::Null]));

    assert!(result.unwrap().is_value_null());
}

#[test]
fn list_with_a_value_not_simplified() {
    let schema = test_schema();
    let simplify = Simplify::new(&schema, &toasty_core::driver::Capability::POSTGRESQL);

    // `x = any([null, 1])`, may match, not simplified
    let result = simplify.simplify_expr_any_op(&any_eq(vec![Value::Null, Value::from(1i64)]));

    assert!(result.is_none());
}
