//! Ordering by a value of a related record (`Query::value_for`).

use crate::prelude::*;
use toasty::Db;
use toasty::stmt::{Expr, List, OrderBy, Page, Query};

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    #[auto]
    id: uuid::Uuid,

    name: String,
}

/// One row per user and key.
#[derive(Debug, toasty::Model)]
struct Setting {
    #[key]
    #[auto]
    id: uuid::Uuid,

    user_id: uuid::Uuid,

    key: String,

    text: Option<String>,

    number: Option<i64>,

    day: Option<jiff::civil::Date>,
}

/// The users, each with a "theme" (text) and a "level" (number) setting or
/// not. `fay` has a "theme" row whose value is null.
async fn setup_users(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(User, Setting)).await;

    for (name, theme, level) in [
        ("ann", Some("dark"), Some(3_i64)),
        ("bob", None, Some(1)),
        ("cid", Some("auto"), None),
        ("dee", Some("light"), Some(3)),
        ("eve", Some("dark"), None),
        ("fay", None, Some(2)),
    ] {
        let user = User::create().name(name).exec(&mut db).await?;
        // A row of another key, which no ordering below may read.
        Setting::create()
            .user_id(user.id)
            .key("lang")
            .text(Some(name.to_string()))
            .number(Some(100))
            .exec(&mut db)
            .await?;
        if theme.is_some() || name == "fay" {
            Setting::create()
                .user_id(user.id)
                .key("theme")
                .text(theme.map(str::to_string))
                .exec(&mut db)
                .await?;
        }
        if let Some(level) = level {
            Setting::create()
                .user_id(user.id)
                .key("level")
                .number(Some(level))
                .exec(&mut db)
                .await?;
            // A date that sorts against the level: later for a lower one.
            Setting::create()
                .user_id(user.id)
                .key("joined")
                .day(Some(jiff::civil::date(2024 - level as i16, 12, 31)))
                .exec(&mut db)
                .await?;
        }
    }

    Ok(db)
}

/// Each user's text setting `key`, `None` without one.
fn text(key: &str) -> Expr<Option<Option<String>>> {
    Query::<List<Setting>>::all()
        .filter(Setting::fields().key().eq(key))
        .value_for(
            Setting::fields().text(),
            Setting::fields().user_id(),
            User::fields().id(),
        )
}

/// Each user's number setting `key`, `None` without one.
fn number(key: &str) -> Expr<Option<Option<i64>>> {
    Query::<List<Setting>>::all()
        .filter(Setting::fields().key().eq(key))
        .value_for(
            Setting::fields().number(),
            Setting::fields().user_id(),
            User::fields().id(),
        )
}

/// Each user's date setting `key`, `None` without one.
fn day(key: &str) -> Expr<Option<Option<jiff::civil::Date>>> {
    Query::<List<Setting>>::all()
        .filter(Setting::fields().key().eq(key))
        .value_for(
            Setting::fields().day(),
            Setting::fields().user_id(),
            User::fields().id(),
        )
}

/// The names in `order`, ties broken by name.
async fn names(db: &mut Db, order: impl Into<OrderBy>) -> Result<Vec<String>> {
    let users: Vec<User> = User::all()
        .order_by(order)
        .order_by(User::fields().name().asc())
        .exec(db)
        .await?;
    Ok(users.into_iter().map(|user| user.name).collect())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_ascending(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(&mut db, text("theme").asc().nulls_last()).await?,
        ["cid", "ann", "eve", "dee", "bob", "fay"]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_descending(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(&mut db, text("theme").desc().nulls_last()).await?,
        ["dee", "ann", "eve", "cid", "bob", "fay"]
    );
    assert_eq!(
        names(&mut db, text("theme").desc().nulls_first()).await?,
        ["bob", "fay", "dee", "ann", "eve", "cid"]
    );

    Ok(())
}

/// A user without a row and a user whose row holds no value both order as
/// null.
#[driver_test(requires(sql))]
pub async fn a_missing_related_row_orders_as_null(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(&mut db, text("theme").asc().nulls_first()).await?,
        ["bob", "fay", "cid", "ann", "eve", "dee"]
    );
    assert_eq!(
        names(&mut db, text("unknown").asc().nulls_last()).await?,
        ["ann", "bob", "cid", "dee", "eve", "fay"]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_number(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(&mut db, number("level").desc().nulls_last()).await?,
        ["ann", "dee", "fay", "bob", "cid", "eve"]
    );

    Ok(())
}

/// A date is compared as the database stores it (as text on SQLite), not
/// decoded.
#[driver_test(requires(sql))]
pub async fn order_by_a_related_date(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(&mut db, day("joined").asc().nulls_last()).await?,
        ["ann", "dee", "fay", "bob", "cid", "eve"]
    );

    Ok(())
}

/// A filter that no record can match orders every user as null.
#[driver_test(requires(sql))]
pub async fn a_related_value_no_record_can_match_orders_as_null(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let never = Query::<List<Setting>>::all()
        .filter(
            Setting::fields()
                .key()
                .eq("theme")
                .and(Setting::fields().key().eq("level")),
        )
        .value_for(
            Setting::fields().text(),
            Setting::fields().user_id(),
            User::fields().id(),
        );
    assert_eq!(
        names(&mut db, never.desc().nulls_last()).await?,
        ["ann", "bob", "cid", "dee", "eve", "fay"]
    );

    Ok(())
}

/// Two related values order the users: equal levels fall to the theme, and
/// equal pairs to the name.
#[driver_test(requires(sql))]
pub async fn order_by_two_related_values(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    assert_eq!(
        names(
            &mut db,
            (
                number("level").asc().nulls_last(),
                text("theme").desc().nulls_last(),
            )
        )
        .await?,
        ["bob", "fay", "dee", "ann", "eve", "cid"]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_with_limit_and_offset(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let mut seen = vec![];
    for offset in [0, 4] {
        let users: Vec<User> = User::all()
            .order_by((
                text("theme").asc().nulls_last(),
                User::fields().name().asc(),
            ))
            .limit(4)
            .offset(offset)
            .exec(&mut db)
            .await?;
        seen.extend(users.into_iter().map(|user| user.name));
    }
    assert_eq!(seen, ["cid", "ann", "eve", "dee", "bob", "fay"]);

    Ok(())
}

/// The filter of the outer query and the subquery's filter combine.
#[driver_test(requires(sql))]
pub async fn order_a_filtered_query_by_a_related_value(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let users: Vec<User> = User::filter(User::fields().name().ne("ann"))
        .order_by((
            number("level").asc().nulls_last(),
            User::fields().name().desc(),
        ))
        .exec(&mut db)
        .await?;
    let names: Vec<_> = users.iter().map(|user| user.name.as_str()).collect();
    assert_eq!(names, ["bob", "fay", "dee", "eve", "cid"]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn cursor_pagination_by_a_related_value_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let err = User::all()
        .order_by(text("theme").asc())
        .paginate(2)
        .exec(&mut db)
        .await
        .map(|_: Page<User>| ())
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_related_value_in_a_filter_is_rejected(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let err = User::filter(text("theme").eq(Some(Some("dark".to_string()))))
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}

#[driver_test(requires(not(sql)))]
pub async fn a_related_value_is_rejected_without_sql(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[key(id, order)]
    struct Entry {
        id: i64,

        order: i64,
    }

    #[derive(Debug, toasty::Model)]
    struct Note {
        #[key]
        id: i64,

        entry: i64,
    }

    let mut db = test.setup_db(models!(Entry, Note)).await;

    let value = Query::<List<Note>>::all().value_for(
        Note::fields().id(),
        Note::fields().entry(),
        Entry::fields().order(),
    );
    let err = Entry::all()
        .filter(Entry::fields().id().eq(0))
        .order_by(value.asc())
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "unexpected error: {err}");

    Ok(())
}

/// Ordering a model by a field of a `belongs_to` parent, ascending and
/// descending. A record without a parent orders as null.
#[driver_test(requires(sql))]
pub async fn order_by_a_belongs_to_field(test: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    struct Company {
        #[key]
        #[auto]
        id: uuid::Uuid,

        name: String,
    }

    #[derive(Debug, toasty::Model)]
    struct Member {
        #[key]
        #[auto]
        id: uuid::Uuid,

        name: String,

        #[index]
        company_id: Option<uuid::Uuid>,

        #[belongs_to(key = company_id, references = id)]
        company: toasty::Deferred<Option<Company>>,
    }

    let mut db = test.setup_db(models!(Company, Member)).await;

    let acme = Company::create().name("acme").exec(&mut db).await?;
    let zeta = Company::create().name("zeta").exec(&mut db).await?;
    for (name, company) in [
        ("ann", Some(&zeta)),
        ("bob", None),
        ("cid", Some(&acme)),
        ("dee", Some(&zeta)),
        ("eve", None),
    ] {
        let mut member = Member::create().name(name);
        if let Some(company) = company {
            member = member.company(company);
        }
        member.exec(&mut db).await?;
    }

    async fn members(db: &mut Db, order: impl Into<OrderBy>) -> Result<Vec<String>> {
        let members: Vec<Member> = Member::all()
            .order_by(order)
            .order_by(Member::fields().name().asc())
            .exec(db)
            .await?;
        Ok(members.into_iter().map(|member| member.name).collect())
    }

    assert_eq!(
        members(
            &mut db,
            Member::fields().company().name().asc().nulls_last()
        )
        .await?,
        ["cid", "ann", "dee", "bob", "eve"]
    );
    assert_eq!(
        members(
            &mut db,
            Member::fields().company().name().desc().nulls_last()
        )
        .await?,
        ["ann", "dee", "cid", "bob", "eve"]
    );
    assert_eq!(
        members(
            &mut db,
            Member::fields().company().name().asc().nulls_first()
        )
        .await?,
        ["bob", "eve", "cid", "ann", "dee"]
    );

    Ok(())
}
