//! What a query update reports about the records it changed: `exec_count`
//! and `exec_returning`.

use crate::prelude::*;

use toasty_core::{
    driver::{Operation, Rows},
    stmt::Statement,
};

#[derive(Debug, toasty::Model)]
struct Item {
    #[key]
    id: i64,

    #[index]
    name: String,

    status: String,
}

async fn create_items(db: &mut toasty::Db, items: &[(i64, &str, &str)]) -> Result<()> {
    for &(id, name, status) in items {
        Item::create()
            .id(id)
            .name(name)
            .status(status)
            .exec(&mut *db)
            .await?;
    }
    Ok(())
}

fn ids(items: &[Item]) -> Vec<i64> {
    let mut ids: Vec<_> = items.iter().map(|item| item.id).collect();
    ids.sort();
    ids
}

/// `exec_count` on an update reports the number of records changed, in one
/// statement.
#[driver_test(requires(sql))]
pub async fn update_exec_count_returns_the_number_changed(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x"), (2, "a", "x"), (3, "b", "x")]).await?;

    t.log().clear();
    let changed = Item::filter_by_name("a")
        .update()
        .status("y")
        .exec_count(&mut db)
        .await?;
    assert_eq!(changed, 2);

    let (op, resp) = t.log().pop();
    assert_struct!(op, Operation::QuerySql({
        stmt: Statement::Update({ returning: None }),
    }));
    assert_struct!(resp, { values: Rows::Count(2) });
    assert!(t.log().is_empty());

    assert_eq!(
        Item::filter_by_name("zzz")
            .update()
            .status("y")
            .exec_count(&mut db)
            .await?,
        0
    );

    Ok(())
}

/// `exec_returning` on an update returns the changed records with their new
/// values.
#[driver_test(requires(sql))]
pub async fn update_exec_returning_returns_the_changed_records(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x"), (2, "a", "x"), (3, "b", "x")]).await?;

    let changed = Item::filter_by_name("a")
        .update()
        .status("y")
        .exec_returning(&mut db)
        .await?;

    assert_eq!(ids(&changed), [1, 2]);
    assert!(
        changed
            .iter()
            .all(|item| item.status == "y" && item.name == "a")
    );

    let unchanged = Item::get_by_id(&mut db, 3).await?;
    assert_eq!(unchanged.status, "x");

    let none = Item::filter_by_name("zzz")
        .update()
        .status("y")
        .exec_returning(&mut db)
        .await?;
    assert!(none.is_empty());

    Ok(())
}

fn compare_and_set(id: i64, expected: &str, next: &str) -> <Item as Model>::UpdateQuery {
    Item::filter(
        Item::fields()
            .id()
            .eq(id)
            .and(Item::fields().status().eq(expected)),
    )
    .update()
    .status(next)
}

/// An update filtered on the expected value is a compare-and-set: the first
/// writer changes one record, the second changes none.
#[driver_test(requires(sql))]
pub async fn update_exec_count_reports_a_compare_and_set(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "pending")]).await?;

    let won = compare_and_set(1, "pending", "active")
        .exec_count(&mut db)
        .await?;
    assert_eq!(won, 1);

    let lost = compare_and_set(1, "pending", "active")
        .exec_count(&mut db)
        .await?;
    assert_eq!(lost, 0);

    Ok(())
}

/// The same compare-and-set with the rows form returns the record the first
/// writer changed, with its new value, and nothing to the second.
#[driver_test(requires(returning_from_update))]
pub async fn update_exec_returning_reports_a_compare_and_set(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "pending")]).await?;

    let won = compare_and_set(1, "pending", "active")
        .exec_returning(&mut db)
        .await?;
    assert_eq!(ids(&won), [1]);
    assert_eq!(won[0].status, "active");

    let lost = compare_and_set(1, "pending", "active")
        .exec_returning(&mut db)
        .await?;
    assert!(lost.is_empty());

    Ok(())
}

/// Inside a transaction, the count and the rows are the transaction's view:
/// they see its own uncommitted writes, and a rollback undoes the update.
#[driver_test(requires(sql))]
pub async fn update_exec_results_are_the_transactions_view(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x"), (2, "a", "x")]).await?;

    let mut tx = db.transaction().await?;

    // Seen only inside the transaction: record 2 leaves the filter.
    let moved = Item::filter_by_id(2)
        .update()
        .name("b")
        .exec_returning(&mut tx)
        .await?;
    assert_eq!(moved[0].name, "b");

    let changed = Item::filter_by_name("a")
        .update()
        .status("y")
        .exec_returning(&mut tx)
        .await?;
    assert_eq!(ids(&changed), [1]);

    assert_eq!(
        Item::filter_by_name("b")
            .update()
            .status("z")
            .exec_count(&mut tx)
            .await?,
        1
    );

    tx.rollback().await?;

    let all = Item::all().exec(&mut db).await?;
    assert_eq!(ids(&all), [1, 2]);
    assert!(
        all.iter()
            .all(|item| item.name == "a" && item.status == "x")
    );

    Ok(())
}
