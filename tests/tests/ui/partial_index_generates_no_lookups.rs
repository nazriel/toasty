// A partial index generates no by-field methods and no upsert: a lookup by
// its fields alone does not imply its predicate, and a partial unique index
// does not make those fields unique across the table.

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

async fn lookups(db: &mut toasty::Db) {
    let _ = Token::get_by_owner_and_name(db, 1, "ci").await;
    let _ = Token::filter_by_owner(1);
    let _ = Token::upsert_by_owner_and_name();
}

fn main() {}
