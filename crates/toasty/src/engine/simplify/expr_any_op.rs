use toasty_core::stmt;

use crate::engine::simplify::Simplify;

impl Simplify<'_> {
    pub(super) fn simplify_expr_any_op(&self, expr_any_op: &stmt::ExprAnyOp) -> Option<stmt::Expr> {
        let stmt::Expr::Value(stmt::Value::List(items)) = &*expr_any_op.rhs else {
            return None;
        };

        // `x <op> any(empty_list)` → `false`, for every `x`, NULL included.
        if items.is_empty() {
            return Some(stmt::Expr::FALSE);
        }

        // `x <op> any(list of NULLs)` → `null`: every comparison is NULL.
        if items.iter().all(stmt::Value::is_null) {
            return Some(stmt::Expr::null());
        }

        None
    }
}
