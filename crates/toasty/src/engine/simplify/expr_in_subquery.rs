use toasty_core::stmt;

use crate::engine::simplify::Simplify;

impl Simplify<'_> {
    pub(super) fn simplify_expr_in_subquery(
        &self,
        expr_in_subquery: &stmt::ExprInSubquery,
    ) -> Option<stmt::Expr> {
        // `x in (empty_query)` → `false` and `x not in (empty_query)` → `true`,
        // for every `x`, NULL included.
        if self.stmt_query_is_empty(&expr_in_subquery.query) {
            return Some(if expr_in_subquery.negated {
                stmt::Expr::TRUE
            } else {
                stmt::Expr::FALSE
            });
        }

        // `x [not] in (select t.c from (values (null), ...) as t)` → `null`:
        // no element of a set of NULLs compares equal to anything, as
        // `x = t.c` over the same derived table simplifies to `null`.
        if let stmt::ExprSet::Select(select) = &expr_in_subquery.query.body
            && select.filter.expr.is_none()
            && let stmt::Returning::Project(projection) = &select.returning
        {
            let column = match projection {
                stmt::Expr::Record(record) if record.len() == 1 => &record[0],
                projection => projection,
            };

            if self
                .scope(&select.source)
                .is_always_null_derived_column(column)
            {
                return Some(stmt::Expr::null());
            }
        }

        None
    }
}
