use toasty_core::stmt::{Direction, Expr, Nulls, OrderBy, OrderByExpr};

fn order(order: Option<Direction>, nulls: Option<Nulls>) -> OrderByExpr {
    OrderByExpr {
        expr: Expr::from(0i64),
        order,
        nulls,
    }
}

#[test]
fn nulls_last_and_nulls_first_set_the_placement() {
    let expr = order(Some(Direction::Asc), None);
    assert_eq!(expr.clone().nulls_last().nulls, Some(Nulls::Last));
    assert_eq!(expr.nulls_first().nulls, Some(Nulls::First));
}

#[test]
fn reverse_flips_an_explicit_null_placement() {
    let mut by = OrderBy::from((
        order(Some(Direction::Asc), Some(Nulls::Last)),
        order(Some(Direction::Desc), Some(Nulls::First)),
        order(None, None),
    ));
    by.reverse();

    assert_eq!(by.exprs[0].order, Some(Direction::Desc));
    assert_eq!(by.exprs[0].nulls, Some(Nulls::First));
    assert_eq!(by.exprs[1].order, Some(Direction::Asc));
    assert_eq!(by.exprs[1].nulls, Some(Nulls::Last));
    assert_eq!(by.exprs[2].order, Some(Direction::Desc));
    assert_eq!(by.exprs[2].nulls, None);
}

#[test]
fn places_nulls_first_follows_the_explicit_placement_or_the_default() {
    // Explicit placement wins over the database default and the direction.
    for nulls_first_on_asc in [true, false] {
        for direction in [None, Some(Direction::Asc), Some(Direction::Desc)] {
            assert!(order(direction, Some(Nulls::First)).places_nulls_first(nulls_first_on_asc));
            assert!(!order(direction, Some(Nulls::Last)).places_nulls_first(nulls_first_on_asc));
        }
    }

    // Without one, the default holds ascending and flips descending.
    assert!(order(None, None).places_nulls_first(true));
    assert!(order(Some(Direction::Asc), None).places_nulls_first(true));
    assert!(!order(Some(Direction::Desc), None).places_nulls_first(true));
    assert!(!order(Some(Direction::Asc), None).places_nulls_first(false));
    assert!(order(Some(Direction::Desc), None).places_nulls_first(false));
}
