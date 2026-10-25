//! Ordering by a value of a related record whose subquery filters on a column
//! that has a storage type of its own (`Query::value_for`): the filter's bind
//! parameter takes the column's type.

use crate::prelude::*;
use toasty::Db;
use toasty::stmt::{List, OrderBy, Query};

#[derive(Debug, PartialEq, toasty::Embed)]
#[column(type = enum("order_by_entry_kind"))]
enum Kind {
    Primary,
    Secondary,
}

#[derive(Debug, toasty::Model)]
struct Person {
    #[key]
    #[auto]
    id: uuid::Uuid,

    name: String,
}

#[derive(Debug, toasty::Model)]
struct Entry {
    #[key]
    #[auto]
    id: uuid::Uuid,

    person_id: uuid::Uuid,

    kind: Kind,

    weight: f64,

    rank: i64,
}

/// `dee` and `ann` have a primary entry (ranks 1 and 2) and a secondary one
/// (rank 0); `bob` has only a secondary entry; `cid` has none. Weights mirror
/// the ranks: `dee` 0.5, `ann` 1.5, `bob` 2.5.
async fn setup_people(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Person, Entry)).await;

    for (name, entries) in [
        (
            "ann",
            vec![(Kind::Primary, 1.5, 2_i64), (Kind::Secondary, 9.5, 0)],
        ),
        ("bob", vec![(Kind::Secondary, 2.5, 5)]),
        ("cid", vec![]),
        (
            "dee",
            vec![(Kind::Primary, 0.5, 1), (Kind::Secondary, 9.5, 0)],
        ),
    ] {
        let person = Person::create().name(name).exec(&mut db).await?;
        for (kind, weight, rank) in entries {
            Entry::create()
                .person_id(person.id)
                .kind(kind)
                .weight(weight)
                .rank(rank)
                .exec(&mut db)
                .await?;
        }
    }

    Ok(db)
}

/// The names in `order`, ties broken by name.
async fn names(db: &mut Db, order: impl Into<OrderBy>) -> Result<Vec<String>> {
    let people: Vec<Person> = Person::all()
        .order_by(order)
        .order_by(Person::fields().name().asc())
        .exec(db)
        .await?;
    Ok(people.into_iter().map(|person| person.name).collect())
}

/// A filter that compares an enum column is bound as the enum, not as text
/// (PostgreSQL has no `<enum> = text`).
#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_filtered_by_an_enum(test: &mut Test) -> Result<()> {
    let mut db = setup_people(test).await?;

    let rank = Query::<List<Entry>>::all()
        .filter(Entry::fields().kind().eq(Kind::Primary))
        .value_for(
            Entry::fields().rank(),
            Entry::fields().person_id(),
            Person::fields().id(),
        );

    assert_eq!(
        names(&mut db, rank.asc().nulls_last()).await?,
        ["dee", "ann", "bob", "cid"]
    );

    Ok(())
}

/// A float carries no storage type of its own, so the filter's parameter takes
/// the column's.
#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_filtered_by_a_float(test: &mut Test) -> Result<()> {
    let mut db = setup_people(test).await?;

    let rank = Query::<List<Entry>>::all()
        .filter(Entry::fields().weight().eq(1.5))
        .value_for(
            Entry::fields().rank(),
            Entry::fields().person_id(),
            Person::fields().id(),
        );

    // Only `ann` has an entry of weight 1.5.
    assert_eq!(
        names(&mut db, rank.asc().nulls_first()).await?,
        ["bob", "cid", "dee", "ann"]
    );

    Ok(())
}
