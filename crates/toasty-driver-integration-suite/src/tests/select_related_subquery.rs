//! A relation predicate (`any`) and a value of a related record
//! (`Query::value_for`) as columns of `.select(...)`. On SQL databases each is
//! a subquery in the statement's SELECT list.

use crate::prelude::*;
use jiff::civil::{Date, date};
use toasty::Db;
use toasty::stmt::{Expr, List, Query, count_all};

#[derive(Debug, PartialEq, toasty::Embed)]
#[column(type = enum("select_related_grade"))]
enum Grade {
    High,
    Low,
}

#[derive(Debug, toasty::Model)]
struct Owner {
    #[key]
    #[auto]
    id: uuid::Uuid,

    name: String,

    #[has_many]
    items: toasty::Deferred<Vec<Item>>,
}

#[derive(Debug, toasty::Model)]
struct Item {
    #[key]
    #[auto]
    id: uuid::Uuid,

    #[index]
    owner_id: uuid::Uuid,

    #[belongs_to(key = owner_id, references = id)]
    owner: toasty::Deferred<Owner>,

    label: String,

    kind: String,

    grade: Grade,

    due: Date,

    note: Option<String>,
}

/// | owner | label  | kind | grade | due        |
/// |-------|--------|------|-------|------------|
/// | ann   | hammer | tool | High  | 2024-03-01 |
/// | ann   | apple  | food | Low   | 2024-01-15 |
/// | bob   | bread  | food | High  | 2023-12-31 |
/// | cid   |        |      |       |            |
async fn setup_owners(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Owner, Item)).await;

    for (name, items) in [
        (
            "ann",
            vec![
                ("hammer", "tool", Grade::High, date(2024, 3, 1)),
                ("apple", "food", Grade::Low, date(2024, 1, 15)),
            ],
        ),
        (
            "bob",
            vec![("bread", "food", Grade::High, date(2023, 12, 31))],
        ),
        ("cid", vec![]),
    ] {
        let owner = Owner::create().name(name).exec(&mut db).await?;
        for (label, kind, grade, due) in items {
            Item::create()
                .owner_id(owner.id)
                .label(label)
                .kind(kind)
                .grade(grade)
                .due(due)
                .exec(&mut db)
                .await?;
        }
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

/// Whether the owner has an item of `kind`.
fn has_kind(kind: &str) -> Expr<bool> {
    Owner::fields().items().any(Item::fields().kind().eq(kind))
}

/// The label of the owner's item of `kind`, `None` without one.
fn label_of_kind(kind: &str) -> Expr<Option<String>> {
    Query::<List<Item>>::all()
        .filter(Item::fields().kind().eq(kind))
        .limit(1)
        .value_for(
            Item::fields().label(),
            Item::fields().owner_id(),
            Owner::fields().id(),
        )
}

#[driver_test(requires(sql))]
pub async fn select_a_relation_predicate(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let rows: Vec<(String, bool)> = Owner::all()
        .select((Owner::fields().name(), has_kind("tool")))
        .order_by(Owner::fields().name().asc())
        .exec(&mut db)
        .await?;
    assert_one_statement(test);
    assert_eq!(
        rows,
        [
            ("ann".to_string(), true),
            ("bob".to_string(), false),
            ("cid".to_string(), false),
        ]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn select_a_related_value(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let rows: Vec<(String, Option<String>)> = Owner::all()
        .select((Owner::fields().name(), label_of_kind("food")))
        .order_by(Owner::fields().name().asc())
        .exec(&mut db)
        .await?;
    assert_one_statement(test);
    assert_eq!(
        rows,
        [
            ("ann".to_string(), Some("apple".to_string())),
            ("bob".to_string(), Some("bread".to_string())),
            ("cid".to_string(), None),
        ]
    );

    Ok(())
}

/// A value stored in another type than its field's (a date stored as text on
/// SQLite) is decoded.
#[driver_test(requires(sql))]
pub async fn select_a_related_value_that_decodes(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let due = Query::<List<Item>>::all()
        .filter(Item::fields().kind().eq("tool"))
        .value_for(
            Item::fields().due(),
            Item::fields().owner_id(),
            Owner::fields().id(),
        );
    let rows: Vec<(String, Option<Date>)> = Owner::all()
        .select((Owner::fields().name(), due))
        .order_by(Owner::fields().name().asc())
        .exec(&mut db)
        .await?;
    assert_one_statement(test);
    assert_eq!(
        rows,
        [
            ("ann".to_string(), Some(date(2024, 3, 1))),
            ("bob".to_string(), None),
            ("cid".to_string(), None),
        ]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn select_a_relation_predicate_and_a_related_value(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let rows: Vec<(String, bool, Option<String>, bool)> =
        Owner::filter(Owner::fields().name().ne("cid"))
            .select((
                Owner::fields().name(),
                has_kind("tool"),
                label_of_kind("tool"),
                has_kind("food"),
            ))
            .order_by(Owner::fields().name().asc())
            .exec(&mut db)
            .await?;
    assert_one_statement(test);
    assert_eq!(
        rows,
        [
            ("ann".to_string(), true, Some("hammer".to_string()), true),
            ("bob".to_string(), false, None, true),
        ]
    );

    Ok(())
}

/// A filter that compares an enum column is bound as the enum, not as text
/// (PostgreSQL has no `<enum> = text`).
#[driver_test(requires(sql))]
pub async fn select_a_related_value_filtered_by_an_enum(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let high = Query::<List<Item>>::all()
        .filter(Item::fields().grade().eq(Grade::High))
        .value_for(
            Item::fields().label(),
            Item::fields().owner_id(),
            Owner::fields().id(),
        );
    let any_high = Owner::fields()
        .items()
        .any(Item::fields().grade().eq(Grade::High));
    let rows: Vec<(String, Option<String>, bool)> = Owner::all()
        .select((Owner::fields().name(), high, any_high))
        .order_by(Owner::fields().name().asc())
        .exec(&mut db)
        .await?;
    assert_one_statement(test);
    assert_eq!(
        rows,
        [
            ("ann".to_string(), Some("hammer".to_string()), true),
            ("bob".to_string(), Some("bread".to_string()), true),
            ("cid".to_string(), None, false),
        ]
    );

    Ok(())
}

/// A related value reads its enclosing row through the correlation key, so a
/// grouped query takes it when the key is a grouping key. A relation
/// predicate reads the relation field, which is never a grouping key.
#[driver_test(requires(sql))]
pub async fn select_related_subqueries_in_a_grouped_query(test: &mut Test) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let mut rows: Vec<(String, Option<String>, u64)> = Owner::all()
        .group_by((Owner::fields().id(), Owner::fields().name()))
        .select((Owner::fields().name(), label_of_kind("food"), count_all()))
        .exec(&mut db)
        .await?;
    assert_one_statement(test);
    rows.sort();
    assert_eq!(
        rows,
        [
            ("ann".to_string(), Some("apple".to_string()), 1),
            ("bob".to_string(), Some("bread".to_string()), 1),
            ("cid".to_string(), None, 1),
        ]
    );

    let err = Owner::all()
        .group_by(Owner::fields().name())
        .select((Owner::fields().name(), label_of_kind("food")))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, Option<String>)>| ())
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    let err = Owner::all()
        .group_by((Owner::fields().id(), Owner::fields().name()))
        .select((Owner::fields().name(), has_kind("tool")))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, bool)>| ())
        .unwrap_err();
    assert!(err.is_invalid_statement(), "unexpected error: {err}");

    Ok(())
}

/// A related value that would read as `Option<Option<T>>` (a nullable field)
/// or decode an embedded type is not supported in a select.
#[driver_test(requires(sql))]
pub async fn select_a_related_value_of_a_nullable_or_embedded_field_is_rejected(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_owners(test).await?;

    let note = Query::<List<Item>>::all().value_for(
        Item::fields().note(),
        Item::fields().owner_id(),
        Owner::fields().id(),
    );
    let err = Owner::all()
        .select((Owner::fields().name(), note))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, Option<Option<String>>)>| ())
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    let grade: Expr<Option<Grade>> = Query::<List<Item>>::all().value_for(
        Item::fields().grade().into(),
        Item::fields().owner_id(),
        Owner::fields().id(),
    );
    let err = Owner::all()
        .select((Owner::fields().name(), grade))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, Option<Grade>)>| ())
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}

#[driver_test(requires(not(sql)))]
pub async fn select_related_subqueries_are_rejected_without_sql(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Shelf {
        #[key]
        #[auto]
        id: uuid::Uuid,

        name: String,

        #[has_many]
        books: toasty::Deferred<Vec<Book>>,
    }

    #[derive(Debug, toasty::Model)]
    struct Book {
        #[key]
        #[auto]
        id: uuid::Uuid,

        #[index]
        shelf_id: uuid::Uuid,

        #[belongs_to(key = shelf_id, references = id)]
        shelf: toasty::Deferred<Shelf>,

        title: String,
    }

    let mut db = test.setup_db(models!(Shelf, Book)).await;

    let any_book = Shelf::fields()
        .books()
        .any(Book::fields().title().eq("Dune"));
    let err = Shelf::all()
        .select((Shelf::fields().name(), any_book))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, bool)>| ())
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    let title = Query::<List<Book>>::all().value_for(
        Book::fields().title(),
        Book::fields().shelf_id(),
        Shelf::fields().id(),
    );
    let err = Shelf::all()
        .select((Shelf::fields().name(), title))
        .exec(&mut db)
        .await
        .map(|_: Vec<(String, Option<String>)>| ())
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}
