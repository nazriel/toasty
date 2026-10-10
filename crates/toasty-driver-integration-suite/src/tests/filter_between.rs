use crate::prelude::*;

/// Filters on a numeric field using `between()`, returning only rows in range.
#[driver_test]
pub async fn filter_between_numeric(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        score: i64,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for score in [10_i64, 20, 30, 40, 50] {
        Item::create().score(score).exec(&mut db).await?;
    }

    let mut items: Vec<_> = Item::filter(Item::fields().score().between(20_i64, 40_i64))
        .exec(&mut db)
        .await?;

    items.sort_by_key(|i| i.score);
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].score, 20);
    assert_eq!(items[1].score, 30);
    assert_eq!(items[2].score, 40);

    Ok(())
}

/// Verifies that `between()` bounds are inclusive on both ends.
#[driver_test]
pub async fn filter_between_inclusive_bounds(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        value: i64,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for v in [1_i64, 5, 10] {
        Item::create().value(v).exec(&mut db).await?;
    }

    // Both boundary values (1 and 10) should be included.
    let items: Vec<_> = Item::filter(Item::fields().value().between(1_i64, 10_i64))
        .exec(&mut db)
        .await?;
    assert_eq!(items.len(), 3);

    // Exact lower bound.
    let items: Vec<_> = Item::filter(Item::fields().value().between(1_i64, 1_i64))
        .exec(&mut db)
        .await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, 1);

    // Exact upper bound.
    let items: Vec<_> = Item::filter(Item::fields().value().between(10_i64, 10_i64))
        .exec(&mut db)
        .await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, 10);

    Ok(())
}

/// Verifies that `between()` returns nothing when the range excludes all rows.
#[driver_test]
pub async fn filter_between_empty_result(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        value: i64,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for v in [1_i64, 2, 3] {
        Item::create().value(v).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(Item::fields().value().between(100_i64, 200_i64))
        .exec(&mut db)
        .await?;

    assert_eq!(items.len(), 0);

    Ok(())
}

/// Filters on a string field using `between()`.
#[driver_test]
pub async fn filter_between_string(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        name: String,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for name in ["apple", "banana", "cherry", "date", "elderberry"] {
        Item::create().name(name).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(Item::fields().name().between("banana", "date"))
        .exec(&mut db)
        .await?;

    assert_eq!(items.len(), 3);
    assert_eq_unordered!(
        items.iter().map(|i| i.name.as_str()),
        ["banana", "cherry", "date"]
    );

    Ok(())
}

/// For DynamoDB: uses `between()` as a key condition on the sort key.
#[driver_test(requires(not(sql)))]
pub async fn filter_between_sort_key(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[key(pk, sk)]
    struct Item {
        pk: i64,
        sk: i64,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for sk in 0..10_i64 {
        Item::create().pk(1).sk(sk).exec(&mut db).await?;
    }

    // between on the sort key (key condition expression path in DynamoDB)
    let mut items: Vec<_> = Item::filter(
        Item::fields()
            .pk()
            .eq(1_i64)
            .and(Item::fields().sk().between(3_i64, 6_i64)),
    )
    .exec(&mut db)
    .await?;

    items.sort_by_key(|i| i.sk);
    assert_eq!(items.len(), 4);
    assert_eq!(items[0].sk, 3);
    assert_eq!(items[1].sk, 4);
    assert_eq!(items[2].sk, 5);
    assert_eq!(items[3].sk, 6);

    Ok(())
}

/// For DynamoDB: uses `between()` as a key condition on a GSI sort key.
#[driver_test(requires(not(sql)))]
pub async fn filter_between_gsi_sort_key(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[key(id)]
    #[index(partition = [player_id], local = [score])]
    struct Match {
        id: String,
        player_id: String,
        score: i64,
    }

    let mut db = test.setup_db(models!(Match)).await;

    toasty::create!(Match::[
        { id: "m1", player_id: "p1", score: 10_i64 },
        { id: "m2", player_id: "p1", score: 20_i64 },
        { id: "m3", player_id: "p1", score: 30_i64 },
        { id: "m4", player_id: "p1", score: 40_i64 },
        { id: "m5", player_id: "p1", score: 50_i64 },
        { id: "m6", player_id: "p2", score: 25_i64 },
    ])
    .exec(&mut db)
    .await?;

    let mut matches: Vec<Match> = Match::filter(
        Match::fields()
            .player_id()
            .eq("p1")
            .and(Match::fields().score().between(20_i64, 40_i64)),
    )
    .exec(&mut db)
    .await?;

    matches.sort_by_key(|m| m.score);
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0].score, 20);
    assert_eq!(matches[1].score, 30);
    assert_eq!(matches[2].score, 40);

    Ok(())
}

/// `between()` on a float column. A float literal carries no storage type of
/// its own, so its bounds take the column's.
#[driver_test]
pub async fn filter_between_f64(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        price: f64,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for price in [0.5_f64, 1.0, 1.5, 2.0, 2.5] {
        Item::create().price(price).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(Item::fields().price().between(1.0_f64, 2.0_f64))
        .exec(&mut db)
        .await?;

    let mut prices: Vec<_> = items.iter().map(|i| i.price).collect();
    prices.sort_by(f64::total_cmp);
    assert_eq!(prices, [1.0, 1.5, 2.0]);

    Ok(())
}

/// `between()` on a nullable float column matches the values inside the
/// range, both bounds included, and never a NULL.
#[driver_test]
pub async fn filter_between_nullable_f64(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        price: Option<f64>,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for price in [
        Some(0.5_f64),
        Some(1.0),
        Some(1.5),
        Some(2.0),
        Some(2.5),
        None,
    ] {
        Item::create().price(price).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(Item::fields().price().between(1.0_f64, 2.0_f64))
        .exec(&mut db)
        .await?;

    let mut prices: Vec<_> = items.iter().map(|i| i.price.unwrap()).collect();
    prices.sort_by(f64::total_cmp);
    assert_eq!(prices, [1.0, 1.5, 2.0]);

    // A range that covers every value still leaves out the NULL.
    let items: Vec<_> = Item::filter(Item::fields().price().between(-1.0e9_f64, 1.0e9_f64))
        .exec(&mut db)
        .await?;
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|i| i.price.is_some()));

    Ok(())
}

/// `between()` on a nullable integer column matches the values inside the
/// range, both bounds included, and never a NULL.
#[driver_test]
pub async fn filter_between_nullable_i64(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        score: Option<i64>,
    }

    let mut db = test.setup_db(models!(Item)).await;

    for score in [Some(10_i64), Some(20), Some(30), Some(40), Some(50), None] {
        Item::create().score(score).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(Item::fields().score().between(20_i64, 40_i64))
        .exec(&mut db)
        .await?;
    assert_eq_unordered!(
        items.iter().map(|i| i.score),
        [&Some(20), &Some(30), &Some(40)]
    );

    let items: Vec<_> = Item::filter(Item::fields().score().between(i64::MIN, i64::MAX))
        .exec(&mut db)
        .await?;
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|i| i.score.is_some()));

    Ok(())
}

/// `between()` on a nullable date column matches the dates inside the range,
/// both bounds included, and never a NULL.
#[driver_test]
pub async fn filter_between_nullable_date(test: &mut Test) -> Result<()> {
    use jiff::civil::Date;

    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        #[auto]
        id: uuid::Uuid,

        due: Option<Date>,
    }

    let mut db = test.setup_db(models!(Item)).await;

    let dates = [
        Date::constant(2024, 1, 31),
        Date::constant(2024, 2, 1),
        Date::constant(2024, 2, 15),
        Date::constant(2024, 2, 29),
        Date::constant(2024, 3, 1),
    ];
    for due in dates.iter().copied().map(Some).chain([None]) {
        Item::create().due(due).exec(&mut db).await?;
    }

    let items: Vec<_> = Item::filter(
        Item::fields()
            .due()
            .between(Date::constant(2024, 2, 1), Date::constant(2024, 2, 29)),
    )
    .exec(&mut db)
    .await?;
    assert_eq_unordered!(
        items.iter().map(|i| i.due),
        [&Some(dates[1]), &Some(dates[2]), &Some(dates[3])]
    );

    let items: Vec<_> = Item::filter(
        Item::fields()
            .due()
            .between(Date::constant(1900, 1, 1), Date::constant(2100, 1, 1)),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(items.len(), 5);
    assert!(items.iter().all(|i| i.due.is_some()));

    Ok(())
}
