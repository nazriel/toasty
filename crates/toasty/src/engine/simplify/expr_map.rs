use super::Simplify;
use crate::engine::eval::Func;
use toasty_core::stmt;

impl Simplify<'_> {
    pub(super) fn simplify_expr_map(&self, expr: &mut stmt::Expr) -> Option<stmt::Expr> {
        let stmt::Expr::Map(expr_map) = expr else {
            unreachable!()
        };

        // `map(list, |x| x.<path>)` → the field of each element. Projecting
        // needs no element type, which an empty list, or one mixing records
        // with NULL and non-NULL fields, does not provide.
        if let stmt::Expr::Value(stmt::Value::List(items)) = &*expr_map.base
            && let stmt::Expr::Project(project) = &*expr_map.map
            && let stmt::Expr::Arg(stmt::ExprArg {
                position: 0,
                nesting: 0,
            }) = &*project.base
        {
            return Some(stmt::Expr::Value(stmt::Value::List(
                items
                    .iter()
                    .map(|item| item.entry(&project.projection).to_value())
                    .collect(),
            )));
        }

        if expr_map.base.is_value() {
            let eval = Func::try_from_stmt(expr, vec![])?;

            let ret = eval.eval_const(self.cx.schema());
            Some(stmt::Expr::Value(ret))
        } else {
            None
        }
    }
}
