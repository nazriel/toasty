//! Aggregates (`count_all`, `count`, `sum`, `avg`, `min`, `max`), `group_by`
//! and `having`.

use crate::prelude::*;
use jiff::civil::{Date, date};
use toasty::Db;
use toasty::stmt::{avg, count, count_all, max, min, sum};

#[derive(Debug, toasty::Model)]
struct Order {
    #[key]
    #[auto]
    id: uuid::Uuid,

    status: String,

    region: String,

    total: i64,

    discount: Option<i64>,

    weight: f64,

    placed_on: Date,
}

/// `count_all`, `count`, `sum`, `avg`, `min`, `max`.
type EveryAggregate = (
    u64,
    u64,
    Option<i64>,
    Option<f64>,
    Option<i64>,
    Option<Date>,
);

/// `count_all`, then `sum`, `avg`, `min` and `max` of one integer field.
type TotalSummary = (u64, Option<i64>, Option<f64>, Option<i64>, Option<i64>);

/// The smallest and largest status, then the earliest and latest date.
type Ranges = (Option<String>, Option<String>, Option<Date>, Option<Date>);

/// | status  | region | total | discount | weight | placed_on  |
/// |---------|--------|-------|----------|--------|------------|
/// | open    | north  | 10    | 1        | 1.5    | 2024-03-01 |
/// | open    | south  | 20    |          | 2.5    | 2024-01-15 |
/// | open    | north  | 30    | 3        | 0.5    | 2024-02-10 |
/// | paid    | north  | 5     |          | 4.0    | 2023-12-31 |
/// | shipped | south  | 7     | 2        | 1.0    | 2024-05-05 |
async fn setup_orders(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Order)).await;

    for (status, region, total, discount, weight, placed_on) in [
        ("open", "north", 10, Some(1), 1.5, date(2024, 3, 1)),
        ("open", "south", 20, None, 2.5, date(2024, 1, 15)),
        ("open", "north", 30, Some(3), 0.5, date(2024, 2, 10)),
        ("paid", "north", 5, None, 4.0, date(2023, 12, 31)),
        ("shipped", "south", 7, Some(2), 1.0, date(2024, 5, 5)),
    ] {
        Order::create()
            .status(status)
            .region(region)
            .total(total)
            .discount(discount)
            .weight(weight)
            .placed_on(placed_on)
            .exec(&mut db)
            .await?;
    }

    test.log().clear();
    Ok(db)
}

/// The query ran as one SQL statement.
#[track_caller]
fn assert_one_statement(test: &mut Test) {
    assert!(test.log().pop_op().is_query_sql());
    assert!(
        test.log().is_empty(),
        "more than one statement: {:#?}",
        test.log()
    );
}

#[driver_test(requires(sql))]
pub async fn every_aggregate_over_an_empty_table(test: &mut Test) -> Result<()> {
    let mut db = test.setup_db(models!(Order)).await;
    test.log().clear();

    let rows: Vec<EveryAggregate> = Order::all()
        .select((
            count_all(),
            count(Order::fields().discount()),
            sum(Order::fields().total()),
            avg(Order::fields().total()),
            min(Order::fields().total()),
            max(Order::fields().placed_on()),
        ))
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [(0, 0, None, None, None, None)]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn every_aggregate_over_records(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<TotalSummary> = Order::all()
        .select((
            count_all(),
            sum(Order::fields().total()),
            avg(Order::fields().total()),
            min(Order::fields().total()),
            max(Order::fields().total()),
        ))
        .exec(&mut db)
        .await?;

    // The mean of an integer field is a float: 72 / 5.
    assert_eq!(rows, [(5, Some(72), Some(14.4), Some(5), Some(30))]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn count_of_a_field_skips_nulls(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(u64, u64, Option<i64>, Option<f64>)> = Order::all()
        .select((
            count_all(),
            count(Order::fields().discount()),
            sum(Order::fields().discount()),
            avg(Order::fields().discount()),
        ))
        .exec(&mut db)
        .await?;

    // Five orders, three with a discount: 1 + 3 + 2.
    assert_eq!(rows, [(5, 3, Some(6), Some(2.0))]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn sum_and_average_of_a_float_field(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(Option<f64>, Option<f64>)> = Order::all()
        .select((sum(Order::fields().weight()), avg(Order::fields().weight())))
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [(Some(9.5), Some(1.9))]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn min_and_max_keep_the_field_type(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<Ranges> = Order::all()
        .select((
            min(Order::fields().status()),
            max(Order::fields().status()),
            min(Order::fields().placed_on()),
            max(Order::fields().placed_on()),
        ))
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [(
            Some("open".to_string()),
            Some("shipped".to_string()),
            Some(date(2023, 12, 31)),
            Some(date(2024, 5, 5)),
        )]
    );
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn aggregates_of_a_filter_that_matches_nothing(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    // An empty `IN` list is known to match nothing before the query runs,
    // yet an aggregate without grouping still yields its one row.
    let rows: Vec<(u64, Option<i64>)> =
        Order::filter(Order::fields().status().in_list(Vec::<String>::new()))
            .select((count_all(), sum(Order::fields().total())))
            .exec(&mut db)
            .await?;

    assert_eq!(rows, [(0, None)]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn aggregates_of_filtered_records(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(u64, Option<i64>)> = Order::filter(Order::fields().region().eq("north"))
        .select((count_all(), max(Order::fields().total())))
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [(3, Some(30))]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_one_field(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(String, u64, Option<i64>, Option<Date>)> = Order::all()
        .group_by(Order::fields().status())
        .select((
            Order::fields().status(),
            count_all(),
            sum(Order::fields().total()),
            min(Order::fields().placed_on()),
        ))
        .order_by(Order::fields().status().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("open".to_string(), 3, Some(60), Some(date(2024, 1, 15))),
            ("paid".to_string(), 1, Some(5), Some(date(2023, 12, 31))),
            ("shipped".to_string(), 1, Some(7), Some(date(2024, 5, 5))),
        ]
    );
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_two_fields(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(String, String, u64, Option<i64>)> = Order::all()
        .group_by((Order::fields().status(), Order::fields().region()))
        .select((
            Order::fields().status(),
            Order::fields().region(),
            count_all(),
            sum(Order::fields().total()),
        ))
        .order_by((
            Order::fields().status().asc(),
            Order::fields().region().asc(),
        ))
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("open".to_string(), "north".to_string(), 2, Some(40)),
            ("open".to_string(), "south".to_string(), 1, Some(20)),
            ("paid".to_string(), "north".to_string(), 1, Some(5)),
            ("shipped".to_string(), "south".to_string(), 1, Some(7)),
        ]
    );
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_date_field(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(Date, u64)> = Order::filter(Order::fields().status().eq("open"))
        .group_by(Order::fields().placed_on())
        .select((Order::fields().placed_on(), count_all()))
        .order_by(Order::fields().placed_on().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            (date(2024, 1, 15), 1),
            (date(2024, 2, 10), 1),
            (date(2024, 3, 1), 1),
        ]
    );
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn having_keeps_the_groups_that_match(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(String, Option<i64>)> = Order::all()
        .group_by(Order::fields().status())
        .having(sum(Order::fields().total()).gt(6_i64))
        .select((Order::fields().status(), sum(Order::fields().total())))
        .order_by(Order::fields().status().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("open".to_string(), Some(60)),
            ("shipped".to_string(), Some(7))
        ]
    );
    assert_one_statement(test);

    // Two conditions, one on a count and one on a grouping key.
    let rows: Vec<String> = Order::all()
        .group_by(Order::fields().status())
        .having(count_all().ge(1_u64))
        .having(Order::fields().status().ne("open"))
        .select(Order::fields().status())
        .order_by(Order::fields().status().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["paid", "shipped"]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_an_aggregate(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let rows: Vec<(String, u64)> = Order::all()
        .group_by(Order::fields().region())
        .select((Order::fields().region(), count_all()))
        .order_by(count_all().desc())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [("north".to_string(), 3), ("south".to_string(), 2)]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_an_aggregate_with_nulls_last_and_a_limit(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    // "paid" has no discount, so its sum is NULL and sorts last.
    let rows: Vec<(String, Option<i64>)> = Order::all()
        .group_by(Order::fields().status())
        .select((Order::fields().status(), sum(Order::fields().discount())))
        .order_by(sum(Order::fields().discount()).desc().nulls_last())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("open".to_string(), Some(4)),
            ("shipped".to_string(), Some(2)),
            ("paid".to_string(), None),
        ]
    );
    assert_one_statement(test);

    let rows: Vec<String> = Order::all()
        .group_by(Order::fields().status())
        .select(Order::fields().status())
        .order_by(sum(Order::fields().discount()).desc().nulls_last())
        .limit(2)
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["open", "shipped"]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_field_neither_grouped_nor_aggregated_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let err = Order::all()
        .group_by(Order::fields().status())
        .select((Order::fields().status(), Order::fields().region()))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    // Without `group_by`, an aggregate makes the whole table one group.
    let err = Order::all()
        .select((Order::fields().status(), count_all()))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    // Ordering reads the groups too.
    let err = Order::all()
        .group_by(Order::fields().status())
        .select(Order::fields().status())
        .order_by(Order::fields().total().asc())
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    assert!(test.log().is_empty());
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_grouped_query_without_select_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let err = Order::all()
        .group_by(Order::fields().status())
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    assert!(test.log().is_empty());
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn an_aggregate_in_a_filter_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_orders(test).await?;

    let err = Order::filter(count_all().gt(1_u64))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    assert!(test.log().is_empty());
    Ok(())
}

#[driver_test(requires(not(sql)))]
pub async fn aggregates_are_rejected_without_sql(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[key(id, order)]
    struct Entry {
        id: i64,

        order: i64,

        total: i64,
    }

    let mut db = test.setup_db(models!(Entry)).await;

    let err = Entry::all()
        .filter(Entry::fields().id().eq(0))
        .select(sum(Entry::fields().total()))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    let err = Entry::all()
        .filter(Entry::fields().id().eq(0))
        .group_by(Entry::fields().order())
        .select(Entry::fields().order())
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, toasty::Embed)]
enum Priority {
    #[column(variant = 1)]
    Low,
    #[column(variant = 2)]
    High,
}

#[derive(Debug, toasty::Embed)]
enum Resolution {
    #[column(variant = 1)]
    Open,
    #[column(variant = 2)]
    Closed { reason: String },
}

#[derive(Debug, toasty::Embed)]
struct Place {
    city: String,
}

#[derive(Debug, toasty::Model)]
struct Ticket {
    #[key]
    #[auto]
    id: uuid::Uuid,

    priority: Priority,

    resolution: Resolution,

    place: Place,

    hours: i64,
}

/// | priority | resolution     | city | hours |
/// |----------|----------------|------|-------|
/// | High     | Open           | Oslo | 1     |
/// | Low      | Closed(done)   | Oslo | 2     |
/// | High     | Closed(dupe)   | Rome | 4     |
async fn setup_tickets(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Ticket)).await;

    for (priority, resolution, city, hours) in [
        (Priority::High, Resolution::Open, "Oslo", 1),
        (
            Priority::Low,
            Resolution::Closed {
                reason: "done".to_string(),
            },
            "Oslo",
            2,
        ),
        (
            Priority::High,
            Resolution::Closed {
                reason: "dupe".to_string(),
            },
            "Rome",
            4,
        ),
    ] {
        Ticket::create()
            .priority(priority)
            .resolution(resolution)
            .place(Place {
                city: city.to_string(),
            })
            .hours(hours)
            .exec(&mut db)
            .await?;
    }

    test.log().clear();
    Ok(db)
}

#[driver_test(requires(sql))]
pub async fn group_by_a_unit_enum(test: &mut Test) -> Result<()> {
    let mut db = setup_tickets(test).await?;

    let mut rows: Vec<(Priority, u64, Option<i64>)> = Ticket::all()
        .group_by(Ticket::fields().priority())
        .select((
            Ticket::fields().priority(),
            count_all(),
            sum(Ticket::fields().hours()),
        ))
        .exec(&mut db)
        .await?;

    // The order of an enum's groups differs by backend.
    rows.sort();
    assert_eq!(
        rows,
        [(Priority::Low, 1, Some(2)), (Priority::High, 2, Some(5))]
    );
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_field_inside_an_embedded_struct(test: &mut Test) -> Result<()> {
    let mut db = setup_tickets(test).await?;

    let rows: Vec<(String, u64)> = Ticket::all()
        .group_by(Ticket::fields().place().city())
        .select((Ticket::fields().place().city(), count_all()))
        .order_by(Ticket::fields().place().city().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [("Oslo".to_string(), 2), ("Rome".to_string(), 1)]);
    assert_one_statement(test);

    let rows: Vec<String> = Ticket::all()
        .group_by(Ticket::fields().place().city())
        .having(Ticket::fields().place().city().ne("Rome"))
        .select(Ticket::fields().place().city())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["Oslo"]);
    assert_one_statement(test);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_data_enum_or_a_whole_struct_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_tickets(test).await?;

    let err = Ticket::all()
        .group_by(Ticket::fields().resolution())
        .select((Ticket::fields().resolution(), count_all()))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    let err = Ticket::all()
        .group_by(Ticket::fields().place())
        .select((Ticket::fields().place(), count_all()))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    assert!(test.log().is_empty());
    Ok(())
}
