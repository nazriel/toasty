use super::{Direction, Expr, Nulls};

/// A single expression within an [`OrderBy`](super::OrderBy) clause, with an
/// optional sort direction and an optional placement of `NULL` values.
///
/// When `order` is `None`, the database default direction is used (typically
/// ascending). When `nulls` is `None`, `NULL` values go where the database
/// puts them by default (see [`Nulls`]).
///
/// # Examples
///
/// ```ignore
/// use toasty_core::stmt::{OrderByExpr, Direction, Expr, Nulls};
///
/// let expr = OrderByExpr {
///     expr: Expr::null(),
///     order: Some(Direction::Desc),
///     nulls: Some(Nulls::Last),
/// };
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByExpr {
    /// The expression to order by.
    pub expr: Expr,

    /// The sort direction, or `None` for the database default.
    pub order: Option<Direction>,

    /// Where `NULL` values go, or `None` for the database default.
    pub nulls: Option<Nulls>,
}

impl OrderByExpr {
    /// Orders by `expr` in the database's default direction, with `NULL`
    /// values where the database puts them by default.
    pub fn new(expr: impl Into<Expr>) -> Self {
        Self {
            expr: expr.into(),
            order: None,
            nulls: None,
        }
    }

    /// Places `NULL` values after every other value, in either direction.
    pub fn nulls_last(mut self) -> Self {
        self.nulls = Some(Nulls::Last);
        self
    }

    /// Places `NULL` values before every other value, in either direction.
    pub fn nulls_first(mut self) -> Self {
        self.nulls = Some(Nulls::First);
        self
    }

    /// Flips the sort direction. `Desc` becomes `Asc`; default (ascending)
    /// and `Asc` become `Desc`. An explicit placement of `NULL` values flips
    /// too, so the reversed order is exactly the original read backwards.
    pub fn reverse(&mut self) {
        self.order = match self.order {
            Some(Direction::Desc) => Some(Direction::Asc),
            _ => Some(Direction::Desc),
        };
        self.nulls = self.nulls.map(Nulls::reverse);
    }

    /// Whether `NULL` values come first under `nulls_first_on_asc`, the
    /// database's default placement for an ascending order.
    pub fn places_nulls_first(&self, nulls_first_on_asc: bool) -> bool {
        match (self.nulls, self.order) {
            (Some(nulls), _) => nulls == Nulls::First,
            (None, Some(Direction::Desc)) => !nulls_first_on_asc,
            (None, _) => nulls_first_on_asc,
        }
    }
}
