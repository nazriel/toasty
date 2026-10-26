use std::ops::{Deref, DerefMut};

use toasty_core::stmt;

/// Represents an item that the engine can select from the database.
///
/// This generalizes `ExprReference` to support both column references and
/// computed expressions like `COUNT(*)`. Using an enum that derives `Hash` and
/// `Eq` allows the planner to continue deduplicating select items via
/// `SelectItems`. An expression that is not `Hash` is held by the
/// `SelectItems` and referenced by index ([`SelectItem::Inline`]).
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq)]
pub(crate) enum SelectItem {
    /// A reference to a column or field — the traditional case.
    ExprReference(stmt::ExprReference),

    /// The `COUNT(*)` aggregate. SQL-only.
    CountStar,

    /// An aggregate over one column, such as `SUM(total)`. SQL-only.
    Aggregate(AggregateItem),

    /// An expression the database computes for each row, such as a relation
    /// predicate (`id IN (SELECT ...)`) or a correlated scalar subquery. The
    /// index points into [`SelectItems::inline`]. SQL-only.
    Inline(usize),
}

/// An aggregate over one column: `COUNT(column)`, `SUM(column)`, ...
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq)]
pub(crate) struct AggregateItem {
    pub(crate) func: AggregateKind,
    pub(crate) arg: stmt::ExprReference,
}

/// The function of an [`AggregateItem`].
#[derive(Debug, Clone, Copy, PartialEq, Hash, Eq)]
pub(crate) enum AggregateKind {
    /// `COUNT(column)`: the rows where the column is not `NULL`.
    Count,

    /// `SUM`, `AVG`, `MIN` or `MAX`.
    Func(stmt::AggregateFunc),
}

impl SelectItem {
    /// Returns the inner `ExprReference`, panicking if this is not one.
    #[track_caller]
    pub(crate) fn as_expr_reference_unwrap(&self) -> &stmt::ExprReference {
        match self {
            SelectItem::ExprReference(r) => r,
            other => panic!("expected ExprReference, got {other:?}"),
        }
    }

    /// Infer the type of this select item given the expression context.
    /// `inline` holds the expressions of [`SelectItem::Inline`] items.
    pub(crate) fn infer_ty(&self, cx: &stmt::ExprContext<'_>, inline: &[stmt::Expr]) -> stmt::Type {
        match self {
            SelectItem::ExprReference(expr_reference) => cx.infer_expr_reference_ty(expr_reference),
            SelectItem::CountStar => stmt::Type::U64,
            SelectItem::Aggregate(item) => match item.func {
                AggregateKind::Count => stmt::Type::U64,
                AggregateKind::Func(stmt::AggregateFunc::Avg) => stmt::Type::F64,
                AggregateKind::Func(_) => cx.infer_expr_reference_ty(&item.arg),
            },
            SelectItem::Inline(index) => cx.infer_expr_ty(&inline[*index], &[]),
        }
    }

    /// The select item for an aggregate expression of the lowered statement,
    /// or `None` when `expr` is not an aggregate. `COUNT(*)` is
    /// [`SelectItem::CountStar`].
    ///
    /// Lowering leaves an aggregate's argument a plain column reference: a
    /// decode cast on the column moves outside the aggregate.
    pub(crate) fn from_aggregate(expr: &stmt::Expr) -> Option<Self> {
        let (func, arg) = match expr {
            stmt::Expr::Func(stmt::ExprFunc::Count(stmt::FuncCount { arg: None, .. })) => {
                return Some(SelectItem::CountStar);
            }
            stmt::Expr::Func(stmt::ExprFunc::Count(stmt::FuncCount {
                arg: Some(arg),
                filter: None,
            })) => (AggregateKind::Count, &**arg),
            stmt::Expr::Func(stmt::ExprFunc::Aggregate(func)) => {
                (AggregateKind::Func(func.func), &*func.arg)
            }
            _ => return None,
        };

        let stmt::Expr::Reference(arg) = arg else {
            panic!("an aggregate's argument must lower to a column; expr={expr:#?}");
        };

        Some(SelectItem::Aggregate(AggregateItem { func, arg: *arg }))
    }

    /// Convert this select item into the corresponding expression. `inline`
    /// holds the expressions of [`SelectItem::Inline`] items.
    pub(crate) fn to_expr(self, inline: &[stmt::Expr]) -> stmt::Expr {
        match self {
            SelectItem::ExprReference(expr_reference) => stmt::Expr::from(expr_reference),
            SelectItem::CountStar => stmt::Expr::count_star(),
            SelectItem::Aggregate(item) => {
                let arg = stmt::Expr::from(item.arg);
                match item.func {
                    AggregateKind::Count => stmt::Expr::count(arg),
                    AggregateKind::Func(func) => stmt::Expr::aggregate(func, arg),
                }
            }
            SelectItem::Inline(index) => inline[index].clone(),
        }
    }
}

impl From<stmt::ExprReference> for SelectItem {
    fn from(value: stmt::ExprReference) -> Self {
        SelectItem::ExprReference(value)
    }
}

/// A set of [`SelectItem`]s backed by an [`IndexSet`](indexmap::IndexSet) for
/// deduplication and index-based lookup.
#[derive(Debug, Default, Clone)]
pub(crate) struct SelectItems {
    items: indexmap::IndexSet<SelectItem>,

    /// The expressions of [`SelectItem::Inline`] items, deduplicated with
    /// `==` (`stmt::Expr` is not `Hash`).
    inline: Vec<stmt::Expr>,
}

impl SelectItems {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The expressions of [`SelectItem::Inline`] items.
    pub(crate) fn inline(&self) -> &[stmt::Expr] {
        &self.inline
    }

    /// The [`SelectItem::Inline`] item for `expr`, or `None` when `expr` is
    /// not selected.
    pub(crate) fn find_inline(&self, expr: &stmt::Expr) -> Option<SelectItem> {
        self.inline
            .iter()
            .position(|inline| inline == expr)
            .map(SelectItem::Inline)
    }

    /// Select an expression the database computes, returning its item.
    pub(crate) fn insert_inline(&mut self, expr: &stmt::Expr) -> SelectItem {
        let item = self.find_inline(expr).unwrap_or_else(|| {
            self.inline.push(expr.clone());
            SelectItem::Inline(self.inline.len() - 1)
        });
        self.items.insert(item);
        item
    }

    /// The expression of each item, in order.
    pub(crate) fn to_exprs(&self) -> impl Iterator<Item = stmt::Expr> + '_ {
        self.items.iter().map(|item| item.to_expr(&self.inline))
    }

    /// Find the index of an `ExprReference` item, returning `None` if not
    /// present.
    pub(crate) fn try_get_index_of_expr_reference(
        &self,
        expr_reference: impl Into<stmt::ExprReference>,
    ) -> Option<usize> {
        let item = SelectItem::ExprReference(expr_reference.into());
        self.items.get_index_of(&item)
    }

    /// Find the index of an `ExprReference` item.
    pub(crate) fn get_index_of_expr_reference(
        &self,
        expr_reference: impl Into<stmt::ExprReference>,
    ) -> usize {
        self.try_get_index_of_expr_reference(expr_reference)
            .unwrap()
    }

    /// Returns `Type::List(Type::Record(field_tys))` where each `field_ty` is
    /// inferred from the corresponding select item.
    pub(crate) fn infer_record_list_ty(&self, cx: &stmt::ExprContext<'_>) -> stmt::Type {
        let field_tys = self
            .items
            .iter()
            .map(|item| item.infer_ty(cx, &self.inline))
            .collect();
        stmt::Type::list(stmt::Type::Record(field_tys))
    }

    /// Extract only the `ExprReference` items from this set.
    ///
    /// Used by the NoSQL path where only column references are valid.
    pub(crate) fn extract_expr_references(&self) -> indexmap::IndexSet<stmt::ExprReference> {
        self.items
            .iter()
            .map(|item| *item.as_expr_reference_unwrap())
            .collect()
    }
}

impl<'a> IntoIterator for &'a SelectItems {
    type Item = &'a SelectItem;
    type IntoIter = indexmap::set::Iter<'a, SelectItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl Deref for SelectItems {
    type Target = indexmap::IndexSet<SelectItem>;

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

impl DerefMut for SelectItems {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.items
    }
}
