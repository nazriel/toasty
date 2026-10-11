//! Aggregate expressions for grouped and summarizing queries.
//!
//! Each function builds a typed [`Expr`] that a list query can project with
//! [`select`](super::Query::select), filter groups by with
//! [`having`](super::Query::having) or sort by with
//! [`order_by`](super::Query::order_by). Without
//! [`group_by`](super::Query::group_by) the aggregates summarize every
//! matching row in one result row; with it, each group yields one row.
//!
//! The result types follow SQL:
//!
//! | Function | Result | Over no rows |
//! |---|---|---|
//! | [`count_all()`] | `u64` | `0` |
//! | [`count(field)`](count) | `u64`, the rows where `field` is not `NULL` | `0` |
//! | [`sum(field)`](sum) | `Option<T>`, the field's type | `None` |
//! | [`avg(field)`](avg) | `Option<f64>`, also for an integer field | `None` |
//! | [`min(field)`](min), [`max(field)`](max) | `Option<T>`, the field's type | `None` |
//!
//! `T` is the field's type with any `Option` removed, so the sum of an
//! `Option<i64>` field is an `Option<i64>`. Aggregates ignore `NULL` values.
//!
//! Aggregates run in the database and are available on SQL databases.

use super::{Expr, Numeric, Path};
use crate::schema::Field;
use toasty_core::stmt;

/// The number of rows: `COUNT(*)`.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Order {
/// #     #[key]
/// #     id: i64,
/// #     status: String,
/// # }
/// use toasty::stmt::count_all;
///
/// // The number of orders in each status.
/// let q = Order::all()
///     .group_by(Order::fields().status())
///     .select((Order::fields().status(), count_all()));
/// ```
pub fn count_all() -> Expr<u64> {
    Expr::from_untyped(stmt::Expr::count_star())
}

/// The number of rows where `field` is not `NULL`: `COUNT(field)`.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Order {
/// #     #[key]
/// #     id: i64,
/// #     shipped_at: Option<i64>,
/// # }
/// use toasty::stmt::count;
///
/// // How many orders have shipped.
/// let q = Order::all().select(count(Order::fields().shipped_at()));
/// ```
pub fn count<M, U>(field: Path<M, U>) -> Expr<u64> {
    Expr::from_untyped(stmt::Expr::count(field.untyped.into_stmt()))
}

/// The sum of the non-null values of `field`: `SUM(field)`.
///
/// The sum has the field's type, so a sum that does not fit that type is an
/// error. It is `None` when no row has a value.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Order {
/// #     #[key]
/// #     id: i64,
/// #     customer: String,
/// #     total: i64,
/// # }
/// use toasty::stmt::sum;
///
/// // Each customer's spend.
/// let q = Order::all()
///     .group_by(Order::fields().customer())
///     .select((Order::fields().customer(), sum(Order::fields().total())));
/// ```
pub fn sum<M, U>(field: Path<M, U>) -> Expr<Option<U::Inner>>
where
    U: Field,
    U::Inner: Numeric,
{
    aggregate(stmt::AggregateFunc::Sum, field)
}

/// The mean of the non-null values of `field`: `AVG(field)`.
///
/// The mean is an `f64`, also for an integer field. It is `None` when no row
/// has a value.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Review {
/// #     #[key]
/// #     id: i64,
/// #     rating: i32,
/// # }
/// use toasty::stmt::avg;
///
/// let q = Review::all().select(avg(Review::fields().rating()));
/// ```
pub fn avg<M, U>(field: Path<M, U>) -> Expr<Option<f64>>
where
    U: Field,
    U::Inner: Numeric,
{
    aggregate(stmt::AggregateFunc::Avg, field)
}

/// The smallest non-null value of `field`: `MIN(field)`.
///
/// The result has the field's type. It is `None` when no row has a value.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Product {
/// #     #[key]
/// #     id: i64,
/// #     category: String,
/// #     price: i64,
/// # }
/// use toasty::stmt::{max, min};
///
/// // The price range of each category.
/// let q = Product::all()
///     .group_by(Product::fields().category())
///     .select((
///         Product::fields().category(),
///         min(Product::fields().price()),
///         max(Product::fields().price()),
///     ));
/// ```
pub fn min<M, U>(field: Path<M, U>) -> Expr<Option<U::Inner>>
where
    U: Field,
{
    aggregate(stmt::AggregateFunc::Min, field)
}

/// The largest non-null value of `field`: `MAX(field)`.
///
/// The result has the field's type. It is `None` when no row has a value.
///
/// # Examples
///
/// ```
/// # #[derive(Debug, toasty::Model)]
/// # struct Session {
/// #     #[key]
/// #     id: i64,
/// #     user_id: i64,
/// #     logged_in_at: i64,
/// # }
/// use toasty::stmt::max;
///
/// // The latest login of each user.
/// let q = Session::all()
///     .group_by(Session::fields().user_id())
///     .select((Session::fields().user_id(), max(Session::fields().logged_in_at())));
/// ```
pub fn max<M, U>(field: Path<M, U>) -> Expr<Option<U::Inner>>
where
    U: Field,
{
    aggregate(stmt::AggregateFunc::Max, field)
}

fn aggregate<M, U, T>(func: stmt::AggregateFunc, field: Path<M, U>) -> Expr<T> {
    Expr::from_untyped(stmt::Expr::aggregate(func, field.untyped.into_stmt()))
}
