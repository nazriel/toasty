use crate::prelude::*;

/// `#[unique(..., filter = "...")]` makes the fields unique among the rows the
/// filter admits and leaves every other row unconstrained.
#[driver_test(requires(partial_index))]
pub async fn partial_unique_index_constrains_only_the_rows_its_filter_admits(
    t: &mut Test,
) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[unique(owner, name, filter = "revoked_at IS NULL")]
    struct Token {
        #[key]
        #[auto]
        id: uuid::Uuid,
        owner: i64,
        name: String,
        revoked_at: Option<i64>,
    }

    let mut db = t.setup_db(models!(Token)).await;

    let index = db.schema().db.tables[0]
        .indices
        .iter()
        .find(|index| !index.primary_key)
        .unwrap();
    assert!(index.unique);
    assert_eq!(index.filter.as_deref(), Some("revoked_at IS NULL"));

    let mut first = Token::create()
        .owner(1)
        .name("deploy")
        .exec(&mut db)
        .await?;

    // A second live token with the same owner and name is refused.
    assert!(
        Token::create()
            .owner(1)
            .name("deploy")
            .exec(&mut db)
            .await
            .is_err()
    );

    // The same name for another owner is not a duplicate.
    Token::create()
        .owner(2)
        .name("deploy")
        .exec(&mut db)
        .await?;

    // Once the first is revoked, the name is free again, and revoked rows
    // never collide with each other.
    first.update().revoked_at(Some(100)).exec(&mut db).await?;
    Token::create()
        .owner(1)
        .name("deploy")
        .exec(&mut db)
        .await?;
    Token::create()
        .owner(1)
        .name("deploy")
        .revoked_at(Some(200))
        .exec(&mut db)
        .await?;

    let tokens: Vec<Token> = Token::filter(
        Token::fields()
            .owner()
            .eq(1)
            .and(Token::fields().name().eq("deploy")),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens.iter().filter(|t| t.revoked_at.is_none()).count(), 1);

    Ok(())
}

/// A query on a partial index's fields that does not repeat its predicate
/// reads every row: the planner never treats the index as covering the table.
#[driver_test(requires(partial_index))]
pub async fn partial_index_leaves_queries_on_its_fields_unchanged(t: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[index(name, filter = "archived = 0")]
    struct Post {
        #[key]
        #[auto]
        id: uuid::Uuid,
        name: String,
        archived: bool,
    }

    let mut db = t.setup_db(models!(Post)).await;

    for archived in [false, true, true] {
        Post::create()
            .name("hello")
            .archived(archived)
            .exec(&mut db)
            .await?;
    }

    let posts: Vec<Post> = Post::filter(Post::fields().name().eq("hello"))
        .exec(&mut db)
        .await?;
    assert_eq!(posts.len(), 3);

    let live: Vec<Post> = Post::filter(
        Post::fields()
            .name()
            .eq("hello")
            .and(Post::fields().archived().eq(false)),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(live.len(), 1);

    Ok(())
}

/// A database without partial indexes rejects a schema that declares one
/// when the schema is built, before anything is created.
#[driver_test(requires(not(partial_index)))]
pub async fn partial_index_is_rejected_without_driver_support(t: &mut Test) -> Result<()> {
    #[derive(Debug, toasty::Model)]
    #[unique(owner, name, filter = "revoked_at IS NULL")]
    struct Token {
        #[key]
        #[auto]
        id: uuid::Uuid,
        owner: i64,
        name: String,
        revoked_at: Option<i64>,
    }

    let err = t.try_setup_db(models!(Token)).await.unwrap_err();

    assert!(err.is_unsupported_feature(), "{err}");
    assert!(
        err.to_string().contains("does not support partial indexes"),
        "{err}"
    );

    Ok(())
}
