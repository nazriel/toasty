use super::{Expr, ExprFunc};

/// An aggregate function other than `COUNT`, applied to one expression.
///
/// `COUNT` is [`FuncCount`](super::FuncCount), which also has a `COUNT(*)`
/// form. Every function here ignores `NULL` inputs and returns `NULL` when it
/// sees no non-null input, including over no rows at all.
///
/// # Examples
///
/// ```text
/// SUM(total)
/// AVG(rating)
/// MIN(created_at)
/// MAX(created_at)
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct FuncAggregate {
    /// Which aggregate to compute.
    pub func: AggregateFunc,

    /// The expression aggregated over the rows of each group.
    pub arg: Box<Expr>,
}

/// The aggregate computed by a [`FuncAggregate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AggregateFunc {
    /// `SUM`: the sum of the values, in the argument's type.
    Sum,

    /// `AVG`: the mean of the values, as a floating-point number.
    Avg,

    /// `MIN`: the smallest value, in the argument's type.
    Min,

    /// `MAX`: the largest value, in the argument's type.
    Max,
}

impl AggregateFunc {
    /// The SQL name of the function.
    pub fn sql_name(self) -> &'static str {
        match self {
            Self::Sum => "SUM",
            Self::Avg => "AVG",
            Self::Min => "MIN",
            Self::Max => "MAX",
        }
    }
}

impl Expr {
    /// Creates an aggregate function expression over `arg`.
    pub fn aggregate(func: AggregateFunc, arg: impl Into<Expr>) -> Self {
        FuncAggregate {
            func,
            arg: Box::new(arg.into()),
        }
        .into()
    }

    /// Creates a `COUNT(arg)` expression, which counts the rows where `arg`
    /// is not `NULL`.
    pub fn count(arg: impl Into<Expr>) -> Self {
        super::FuncCount {
            arg: Some(Box::new(arg.into())),
            filter: None,
        }
        .into()
    }

    /// Returns `true` if this expression is an aggregate function call
    /// (`COUNT`, `SUM`, `AVG`, `MIN` or `MAX`).
    pub fn is_aggregate(&self) -> bool {
        matches!(
            self,
            Self::Func(ExprFunc::Count(_)) | Self::Func(ExprFunc::Aggregate(_))
        )
    }
}

impl From<FuncAggregate> for ExprFunc {
    fn from(value: FuncAggregate) -> Self {
        Self::Aggregate(value)
    }
}

impl From<FuncAggregate> for Expr {
    fn from(value: FuncAggregate) -> Self {
        Self::Func(value.into())
    }
}
