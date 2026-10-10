/// Where an [`OrderByExpr`](super::OrderByExpr) places `NULL` values.
///
/// When an ordering expression leaves this unset, the database's default
/// applies: SQLite and MySQL sort `NULL` before every other value, PostgreSQL
/// after, and a descending order reverses either.
///
/// # Examples
///
/// ```
/// use toasty_core::stmt::Nulls;
///
/// let nulls = Nulls::Last;
/// assert_eq!(nulls.reverse(), Nulls::First);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Nulls {
    /// `NULL` values come before every other value.
    First,
    /// `NULL` values come after every other value.
    Last,
}

impl Nulls {
    /// The opposite placement: `First` becomes `Last` and `Last` becomes
    /// `First`.
    pub fn reverse(self) -> Self {
        match self {
            Self::First => Self::Last,
            Self::Last => Self::First,
        }
    }
}
