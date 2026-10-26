//! `.select(...)` of a field read through `belongs_to` relations, which a
//! SQL database reads with a join in the query's own statement.

use crate::prelude::*;
use toasty::Db;
use toasty::stmt::{count_all, max};
use toasty_core::driver::Operation;
use toasty_core::stmt::JoinOp;

#[derive(Debug, toasty::Model)]
struct Author {
    #[key]
    #[auto]
    id: uuid::Uuid,

    name: String,

    rank: i64,
}

#[derive(Debug, toasty::Model)]
struct Post {
    #[key]
    #[auto]
    id: uuid::Uuid,

    title: String,

    #[index]
    author_id: uuid::Uuid,

    #[belongs_to(key = author_id, references = id)]
    author: toasty::Deferred<Author>,

    #[index]
    editor_id: Option<uuid::Uuid>,

    #[belongs_to(key = editor_id, references = id)]
    editor: toasty::Deferred<Option<Author>>,
}

#[derive(Debug, toasty::Model)]
struct Comment {
    #[key]
    #[auto]
    id: uuid::Uuid,

    body: String,

    #[index]
    post_id: uuid::Uuid,

    #[belongs_to(key = post_id, references = id)]
    post: toasty::Deferred<Post>,
}

/// | post  | author | editor |
/// |-------|--------|--------|
/// | alpha | ann    | bob    |
/// | beta  | ann    |        |
/// | gamma | bob    | ann    |
/// | delta | ann    |        |
///
/// Each post has one comment, whose body is the post's title.
async fn setup_posts(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(Author, Post, Comment)).await;

    let ann = Author::create().name("ann").rank(2).exec(&mut db).await?;
    let bob = Author::create().name("bob").rank(1).exec(&mut db).await?;

    for (title, author, editor) in [
        ("alpha", &ann, Some(&bob)),
        ("beta", &ann, None),
        ("gamma", &bob, Some(&ann)),
        ("delta", &ann, None),
    ] {
        let post = Post::create()
            .title(title)
            .author(author)
            .editor_id(editor.map(|editor| editor.id))
            .exec(&mut db)
            .await?;
        Comment::create()
            .body(title)
            .post(&post)
            .exec(&mut db)
            .await?;
    }

    test.log().clear();
    Ok(db)
}

/// The query ran as one SQL statement; returns the kinds of its joins.
#[track_caller]
fn one_statement_joins(test: &mut Test) -> Vec<&'static str> {
    let Operation::QuerySql(query_sql) = test.log().pop_op() else {
        panic!("expected a SQL query");
    };
    assert!(
        test.log().is_empty(),
        "more than one statement: {:#?}",
        test.log()
    );

    let select = query_sql.stmt.query_select_unwrap();
    select.source.as_table_unwrap().from[0]
        .joins
        .iter()
        .map(|join| match join.constraint {
            JoinOp::Inner(_) => "inner",
            JoinOp::Left(_) => "left",
        })
        .collect()
}

#[driver_test(requires(sql))]
pub async fn a_field_of_a_required_relation_is_read_with_an_inner_join(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<(String, String)> = Post::all()
        .select((Post::fields().title(), Post::fields().author().name()))
        .order_by(Post::fields().title().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("alpha".to_string(), "ann".to_string()),
            ("beta".to_string(), "ann".to_string()),
            ("delta".to_string(), "ann".to_string()),
            ("gamma".to_string(), "bob".to_string()),
        ]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_field_of_an_optional_relation_is_none_without_a_record(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<(String, Option<String>)> = Post::all()
        .select((
            Post::fields().title(),
            Post::fields().editor().name().nullable(),
        ))
        .order_by(Post::fields().title().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("alpha".to_string(), Some("bob".to_string())),
            ("beta".to_string(), None),
            ("delta".to_string(), None),
            ("gamma".to_string(), Some("ann".to_string())),
        ]
    );
    assert_eq!(one_statement_joins(test), ["left"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_filtered_query_reads_a_related_field(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<String> = Post::filter(Post::fields().title().eq("gamma"))
        .select(Post::fields().author().name())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["bob"]);
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_relation_read_twice_is_joined_once(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<(String, i64)> = Post::filter(Post::fields().title().eq("alpha"))
        .select((
            Post::fields().author().name(),
            Post::fields().author().rank(),
        ))
        .exec(&mut db)
        .await?;

    assert_eq!(rows, [("ann".to_string(), 2)]);
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_field_two_relations_away_is_read_with_two_joins(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<(String, String, Option<String>)> = Comment::all()
        .select((
            Comment::fields().body(),
            Comment::fields().post().author().name(),
            Comment::fields().post().editor().name().nullable(),
        ))
        .order_by(Comment::fields().body().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            (
                "alpha".to_string(),
                "ann".to_string(),
                Some("bob".to_string())
            ),
            ("beta".to_string(), "ann".to_string(), None),
            ("delta".to_string(), "ann".to_string(), None),
            (
                "gamma".to_string(),
                "bob".to_string(),
                Some("ann".to_string())
            ),
        ]
    );
    // comment → post, post → author, post → editor
    assert_eq!(one_statement_joins(test), ["inner", "inner", "left"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_field(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<String> = Post::all()
        .select(Post::fields().title())
        .order_by((
            Post::fields().author().rank().asc(),
            Post::fields().title().asc(),
        ))
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["gamma", "alpha", "beta", "delta"]);
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_field_of_a_required_relation(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<(String, u64, Option<String>)> = Post::all()
        .group_by(Post::fields().author().name())
        .select((
            Post::fields().author().name(),
            count_all(),
            max(Post::fields().title()),
        ))
        .order_by(count_all().desc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("ann".to_string(), 3, Some("delta".to_string())),
            ("bob".to_string(), 1, Some("gamma".to_string())),
        ]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_field_of_an_optional_relation(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    // The posts without an editor form their own group.
    let rows: Vec<(Option<String>, u64)> = Post::all()
        .group_by(Post::fields().editor().name().nullable())
        .select((Post::fields().editor().name().nullable(), count_all()))
        .order_by(Post::fields().editor().name().asc().nulls_last())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            (Some("ann".to_string()), 1),
            (Some("bob".to_string()), 1),
            (None, 2),
        ]
    );
    assert_eq!(one_statement_joins(test), ["left"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn having_on_a_group_of_a_related_field(test: &mut Test) -> Result<()> {
    let mut db = setup_posts(test).await?;

    let rows: Vec<String> = Comment::all()
        .group_by(Comment::fields().post().author().name())
        .having(count_all().gt(1_u64))
        .select(Comment::fields().post().author().name())
        .exec(&mut db)
        .await?;

    assert_eq!(rows, ["ann"]);
    assert_eq!(one_statement_joins(test), ["inner", "inner"]);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, toasty::Embed)]
enum Standing {
    #[column(variant = 1)]
    Active,
    #[column(variant = 2)]
    Suspended,
}

#[derive(Debug, PartialEq, toasty::Embed)]
struct Address {
    city: String,
}

#[derive(Debug, PartialEq, toasty::Embed)]
enum Contact {
    #[column(variant = 1)]
    Email { address: String },
    #[column(variant = 2)]
    Phone { number: String },
    #[column(variant = 3)]
    Unlisted,
}

#[derive(Debug, toasty::Embed)]
struct Profile {
    headline: String,
    notes: toasty::Deferred<String>,
}

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    #[auto]
    id: uuid::Uuid,

    email: String,

    standing: Standing,

    address: Address,

    contact: Contact,

    profile: Profile,
}

#[derive(Debug, toasty::Model)]
struct Member {
    #[key]
    #[auto]
    id: uuid::Uuid,

    name: String,

    #[index]
    user_id: uuid::Uuid,

    #[belongs_to(key = user_id, references = id)]
    user: toasty::Deferred<User>,

    #[index]
    sponsor_id: Option<uuid::Uuid>,

    #[belongs_to(key = sponsor_id, references = id)]
    sponsor: toasty::Deferred<Option<User>>,
}

/// A member's name, then its sponsor's standing, contact and address.
type SponsorEmbeds = (String, Option<Standing>, Option<Contact>, Option<Address>);

/// | user | email   | standing  | city | contact          |
/// |------|---------|-----------|------|------------------|
/// | ann  | ann@ex  | Active    | Oslo | Email(ann@ex)    |
/// | bob  | bob@ex  | Suspended | Rome | Phone(555)       |
/// | cid  | cid@ex  | Active    | Oslo | Unlisted         |
///
/// | member | user | sponsor |
/// |--------|------|---------|
/// | m1     | ann  | bob     |
/// | m2     | bob  |         |
/// | m3     | cid  | ann     |
/// | m4     | ann  |         |
async fn setup_members(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(User, Member)).await;

    let mut users = Vec::new();
    for (name, standing, city, contact) in [
        (
            "ann",
            Standing::Active,
            "Oslo",
            Contact::Email {
                address: "ann@ex".to_string(),
            },
        ),
        (
            "bob",
            Standing::Suspended,
            "Rome",
            Contact::Phone {
                number: "555".to_string(),
            },
        ),
        ("cid", Standing::Active, "Oslo", Contact::Unlisted),
    ] {
        let user = User::create()
            .email(format!("{name}@ex"))
            .standing(standing)
            .address(Address {
                city: city.to_string(),
            })
            .contact(contact)
            .profile(Profile {
                headline: format!("{name}'s headline"),
                notes: format!("{name}'s notes").into(),
            })
            .exec(&mut db)
            .await?;
        users.push(user);
    }
    let [ann, bob, cid] = &users[..] else {
        unreachable!()
    };

    for (name, user, sponsor) in [
        ("m1", ann, Some(bob)),
        ("m2", bob, None),
        ("m3", cid, Some(ann)),
        ("m4", ann, None),
    ] {
        Member::create()
            .name(name)
            .user(user)
            .sponsor_id(sponsor.map(|sponsor| sponsor.id))
            .exec(&mut db)
            .await?;
    }

    test.log().clear();
    Ok(db)
}

#[driver_test(requires(sql))]
pub async fn a_unit_enum_of_a_required_relation_is_read_with_an_inner_join(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_members(test).await?;

    let rows: Vec<(String, String, Standing)> = Member::all()
        .select((
            Member::fields().name(),
            Member::fields().user().email(),
            Member::fields().user().standing(),
        ))
        .order_by(Member::fields().name().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("m1".to_string(), "ann@ex".to_string(), Standing::Active),
            ("m2".to_string(), "bob@ex".to_string(), Standing::Suspended),
            ("m3".to_string(), "cid@ex".to_string(), Standing::Active),
            ("m4".to_string(), "ann@ex".to_string(), Standing::Active),
        ]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn embeds_of_an_optional_relation_are_none_without_a_record(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_members(test).await?;

    let standing: toasty::stmt::Path<Member, Standing> =
        Member::fields().sponsor().standing().into();
    let contact: toasty::stmt::Path<Member, Contact> = Member::fields().sponsor().contact().into();
    let address: toasty::stmt::Path<Member, Address> = Member::fields().sponsor().address().into();
    let rows: Vec<SponsorEmbeds> = Member::all()
        .select((
            Member::fields().name(),
            standing.nullable(),
            contact.nullable(),
            address.nullable(),
        ))
        .order_by(Member::fields().name().asc())
        .exec(&mut db)
        .await?;

    let oslo = || Address {
        city: "Oslo".to_string(),
    };
    assert_eq!(
        rows,
        [
            (
                "m1".to_string(),
                Some(Standing::Suspended),
                Some(Contact::Phone {
                    number: "555".to_string()
                }),
                Some(Address {
                    city: "Rome".to_string()
                }),
            ),
            ("m2".to_string(), None, None, None),
            (
                "m3".to_string(),
                Some(Standing::Active),
                Some(Contact::Email {
                    address: "ann@ex".to_string()
                }),
                Some(oslo()),
            ),
            ("m4".to_string(), None, None, None),
        ]
    );
    assert_eq!(one_statement_joins(test), ["left"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_field_inside_an_embedded_struct_of_a_relation_is_joined(
    test: &mut Test,
) -> Result<()> {
    let mut db = setup_members(test).await?;

    let rows: Vec<(String, String)> = Member::all()
        .select((
            Member::fields().name(),
            Member::fields().user().address().city(),
        ))
        .order_by(Member::fields().name().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            ("m1".to_string(), "Oslo".to_string()),
            ("m2".to_string(), "Rome".to_string()),
            ("m3".to_string(), "Oslo".to_string()),
            ("m4".to_string(), "Oslo".to_string()),
        ]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);

    let rows: Vec<Address> = Member::filter(Member::fields().name().eq("m2"))
        .select(Member::fields().user().address())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [Address {
            city: "Rome".to_string()
        }]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn a_data_enum_of_a_relation_is_read_with_a_join(test: &mut Test) -> Result<()> {
    let mut db = setup_members(test).await?;

    let rows: Vec<(String, Contact)> = Member::filter(Member::fields().name().ne("m4"))
        .select((Member::fields().name(), Member::fields().user().contact()))
        .order_by(Member::fields().name().asc())
        .exec(&mut db)
        .await?;

    assert_eq!(
        rows,
        [
            (
                "m1".to_string(),
                Contact::Email {
                    address: "ann@ex".to_string()
                }
            ),
            (
                "m2".to_string(),
                Contact::Phone {
                    number: "555".to_string()
                }
            ),
            ("m3".to_string(), Contact::Unlisted),
        ]
    );
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn an_embed_with_a_deferred_field_keeps_its_own_statement(test: &mut Test) -> Result<()> {
    let mut db = setup_members(test).await?;

    let rows: Vec<Profile> = Member::filter(Member::fields().name().eq("m2"))
        .select(Member::fields().user().profile())
        .exec(&mut db)
        .await?;

    let [profile] = &rows[..] else {
        panic!("expected one row: {rows:#?}");
    };
    assert_eq!(profile.headline, "bob's headline");
    assert!(profile.notes.is_unloaded());
    assert!(test.log().len() > 1, "expected the user's own statement");
    Ok(())
}

#[driver_test(requires(sql))]
pub async fn group_by_a_unit_enum_of_a_relation(test: &mut Test) -> Result<()> {
    let mut db = setup_members(test).await?;

    let mut rows: Vec<(Standing, u64)> = Member::all()
        .group_by(Member::fields().user().standing())
        .select((Member::fields().user().standing(), count_all()))
        .exec(&mut db)
        .await?;

    // The order of an enum's groups differs by backend.
    rows.sort();
    assert_eq!(rows, [(Standing::Active, 3), (Standing::Suspended, 1)]);
    assert_eq!(one_statement_joins(test), ["inner"]);

    let mut rows: Vec<(String, u64)> = Member::all()
        .group_by(Member::fields().user().address().city())
        .select((Member::fields().user().address().city(), count_all()))
        .exec(&mut db)
        .await?;

    rows.sort();
    assert_eq!(rows, [("Oslo".to_string(), 3), ("Rome".to_string(), 1)]);
    assert_eq!(one_statement_joins(test), ["inner"]);
    Ok(())
}
