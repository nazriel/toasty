//! What a query delete reports about the records it removed: `exec_count`
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

/// `exec_count` on a delete reports the number of records removed, in one
/// statement.
#[driver_test(requires(sql))]
pub async fn delete_exec_count_returns_the_number_removed(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x"), (2, "a", "x"), (3, "b", "x")]).await?;

    t.log().clear();
    let removed = Item::filter_by_name("a")
        .delete()
        .exec_count(&mut db)
        .await?;
    assert_eq!(removed, 2);

    let (op, resp) = t.log().pop();
    assert_struct!(op, Operation::QuerySql({
        stmt: Statement::Delete({ returning: None }),
    }));
    assert_struct!(resp, { values: Rows::Count(2) });
    assert!(t.log().is_empty());

    let left = Item::all().exec(&mut db).await?;
    assert_eq!(ids(&left), [3]);

    Ok(())
}

/// The count is what the delete removed, not what matched before it: a second
/// delete with the same filter removes nothing.
#[driver_test(requires(sql))]
pub async fn delete_exec_count_returns_zero_when_nothing_matches(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x")]).await?;

    assert_eq!(
        Item::filter_by_name("a")
            .delete()
            .exec_count(&mut db)
            .await?,
        1
    );
    assert_eq!(
        Item::filter_by_name("a")
            .delete()
            .exec_count(&mut db)
            .await?,
        0
    );
    assert_eq!(
        Item::filter_by_name("zzz")
            .delete()
            .exec_count(&mut db)
            .await?,
        0
    );

    Ok(())
}

/// `exec_returning` on a delete returns the removed records with the values
/// they had, from one `DELETE ... RETURNING`.
#[driver_test(requires(returning_from_delete))]
pub async fn delete_exec_returning_returns_the_removed_records(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(
        &mut db,
        &[(1, "a", "old"), (2, "a", "older"), (3, "b", "x")],
    )
    .await?;

    t.log().clear();
    let removed: Vec<Item> = Item::filter_by_name("a")
        .delete()
        .exec_returning(&mut db)
        .await?;

    assert_eq!(ids(&removed), [1, 2]);
    let mut statuses: Vec<_> = removed.iter().map(|item| item.status.as_str()).collect();
    statuses.sort();
    assert_eq!(statuses, ["old", "older"]);
    assert!(removed.iter().all(|item| item.name == "a"));

    let (op, _) = t.log().pop();
    assert_struct!(op, Operation::QuerySql({
        stmt: Statement::Delete({ returning: Some(_) }),
    }));
    assert!(t.log().is_empty());

    let left = Item::all().exec(&mut db).await?;
    assert_eq!(ids(&left), [3]);

    Ok(())
}

/// A delete that removes nothing returns no records.
#[driver_test(requires(returning_from_delete))]
pub async fn delete_exec_returning_is_empty_when_nothing_matches(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x")]).await?;

    let removed: Vec<Item> = Item::filter_by_name("zzz")
        .delete()
        .exec_returning(&mut db)
        .await?;
    assert!(removed.is_empty());
    assert_eq!(Item::all().exec(&mut db).await?.len(), 1);

    Ok(())
}

/// Without `DELETE ... RETURNING` the rows form is refused before anything is
/// removed; the count form still works.
#[driver_test(requires(and(sql, not(returning_from_delete))))]
pub async fn delete_exec_returning_is_unsupported_without_delete_returning(
    t: &mut Test,
) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x")]).await?;

    let error = Item::filter_by_name("a")
        .delete()
        .exec_returning::<Item>(&mut db)
        .await
        .unwrap_err();
    assert!(error.is_unsupported_feature(), "unexpected error: {error}");
    assert_eq!(Item::all().exec(&mut db).await?.len(), 1);

    assert_eq!(
        Item::filter_by_name("a")
            .delete()
            .exec_count(&mut db)
            .await?,
        1
    );

    Ok(())
}

/// Naming a model other than the one the delete removes is an error, and
/// nothing is removed.
#[driver_test(requires(sql))]
pub async fn delete_exec_returning_rejects_another_model(t: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Other {
        #[key]
        id: i64,
    }

    let mut db = t.setup_db(models!(Item, Other)).await;
    create_items(&mut db, &[(1, "a", "x")]).await?;

    let error = Item::filter_by_name("a")
        .delete()
        .exec_returning::<Other>(&mut db)
        .await
        .unwrap_err();
    assert!(error.is_invalid_statement(), "unexpected error: {error}");
    assert_eq!(Item::all().exec(&mut db).await?.len(), 1);

    Ok(())
}

/// Inside a transaction, the count and the rows are the transaction's view:
/// they see its own uncommitted writes, and a rollback undoes the delete.
#[driver_test(requires(returning_from_delete))]
pub async fn delete_exec_results_are_the_transactions_view(t: &mut Test) -> Result<()> {
    let mut db = t.setup_db(models!(Item)).await;
    create_items(&mut db, &[(1, "a", "x"), (2, "a", "x")]).await?;

    let mut tx = db.transaction().await?;

    // Seen only inside the transaction: record 2 leaves the filter.
    Item::filter_by_id(2)
        .update()
        .name("b")
        .exec(&mut tx)
        .await?;

    let removed: Vec<Item> = Item::filter_by_name("a")
        .delete()
        .exec_returning(&mut tx)
        .await?;
    assert_eq!(ids(&removed), [1]);

    assert_eq!(
        Item::filter_by_name("b")
            .delete()
            .exec_count(&mut tx)
            .await?,
        1
    );

    tx.rollback().await?;

    let all = Item::all().exec(&mut db).await?;
    assert_eq!(ids(&all), [1, 2]);
    assert!(all.iter().all(|item| item.name == "a"));

    Ok(())
}
