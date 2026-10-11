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
