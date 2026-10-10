//! `nulls_first()` and `nulls_last()` on an ordering.

use crate::prelude::*;
use toasty::Db;
use toasty::stmt::{OrderBy, Page};

#[derive(Debug, toasty::Model)]
struct Task {
    #[key]
    #[auto]
    id: uuid::Uuid,

    title: String,

    priority: Option<i64>,
}

async fn setup_tasks(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Task)).await;

    for (title, priority) in [
        ("a", Some(2_i64)),
        ("b", None),
        ("c", Some(1)),
        ("d", None),
        ("e", Some(3)),
    ] {
        Task::create()
            .title(title)
            .priority(priority)
            .exec(&mut db)
            .await?;
    }

    Ok(db)
}

/// The titles in `order`, ties broken by title.
async fn titles(db: &mut Db, order: impl Into<OrderBy>) -> Result<Vec<String>> {
    let tasks: Vec<Task> = Task::all()
        .order_by(order)
        .order_by(Task::fields().title().asc())
        .exec(db)
        .await?;
    Ok(tasks.into_iter().map(|task| task.title).collect())
}

#[driver_test(requires(sql))]
pub async fn nulls_last_ascending(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let order = Task::fields().priority().asc().nulls_last();
    assert_eq!(titles(&mut db, order).await?, ["c", "a", "e", "b", "d"]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn nulls_last_descending(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let order = Task::fields().priority().desc().nulls_last();
    assert_eq!(titles(&mut db, order).await?, ["e", "a", "c", "b", "d"]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn nulls_first_ascending(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let order = Task::fields().priority().asc().nulls_first();
    assert_eq!(titles(&mut db, order).await?, ["b", "d", "c", "a", "e"]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn nulls_first_descending(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let order = Task::fields().priority().desc().nulls_first();
    assert_eq!(titles(&mut db, order).await?, ["b", "d", "e", "a", "c"]);

    Ok(())
}

/// A null placement on a column without nulls changes nothing.
#[driver_test(requires(sql))]
pub async fn nulls_last_on_a_column_without_nulls(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let order = Task::fields().title().desc().nulls_last();
    assert_eq!(titles(&mut db, order).await?, ["e", "d", "c", "b", "a"]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn nulls_last_with_limit_and_offset(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    let page = |offset| {
        Task::all()
            .order_by((
                Task::fields().priority().asc().nulls_last(),
                Task::fields().title().asc(),
            ))
            .limit(2)
            .offset(offset)
    };

    let mut seen = vec![];
    for offset in [0, 2, 4] {
        let tasks: Vec<Task> = page(offset).exec(&mut db).await?;
        seen.extend(tasks.into_iter().map(|task| task.title));
    }
    assert_eq!(seen, ["c", "a", "e", "b", "d"]);

    Ok(())
}

/// Cursor pagination resumes after the cursor in the requested null
/// placement, forwards and backwards, whichever placement the database uses
/// by default.
#[driver_test(requires(and(sql, backward_pagination)))]
pub async fn nulls_placement_with_cursor_pagination(test: &mut Test) -> Result<()> {
    let mut db = setup_tasks(test).await?;

    for (order, expected) in [
        (
            Task::fields().priority().asc().nulls_last(),
            ["c", "a", "e", "b", "d"],
        ),
        (
            Task::fields().priority().desc().nulls_last(),
            ["e", "a", "c", "b", "d"],
        ),
        (
            Task::fields().priority().asc().nulls_first(),
            ["b", "d", "c", "a", "e"],
        ),
        (
            Task::fields().priority().desc().nulls_first(),
            ["b", "d", "e", "a", "c"],
        ),
    ] {
        let mut page: Page<Task> = Task::all()
            .order_by((order, Task::fields().title().asc()))
            .paginate(2)
            .exec(&mut db)
            .await?;

        let mut pages = vec![
            page.items
                .iter()
                .map(|task| task.title.clone())
                .collect::<Vec<_>>(),
        ];
        while let Some(next) = page.next(&mut db).await? {
            pages.push(next.items.iter().map(|task| task.title.clone()).collect());
            page = next;
        }
        assert_eq!(pages.concat(), expected);

        // Back from the last page to the first.
        let mut back = vec![];
        while let Some(prev) = page.prev(&mut db).await? {
            back.push(
                prev.items
                    .iter()
                    .map(|task| task.title.clone())
                    .collect::<Vec<_>>(),
            );
            page = prev;
        }
        back.reverse();
        let last = pages.pop().unwrap();
        assert_eq!(back.concat(), pages.concat());
        assert!(!last.is_empty());
    }

    Ok(())
}

#[driver_test(requires(not(sql)))]
pub async fn nulls_placement_is_rejected_without_sql(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[key(id, order)]
    struct Entry {
        id: i64,

        order: i64,
    }

    let mut db = test.setup_db(models!(Entry)).await;

    let err = Entry::all()
        .filter(Entry::fields().id().eq(0))
        .order_by(Entry::fields().order().asc().nulls_last())
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}
