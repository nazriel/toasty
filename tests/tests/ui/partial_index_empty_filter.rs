// A partial index's `filter` must be a SQL predicate; an empty or blank
// string is rejected, and so is a second `filter`.

#[derive(Debug, toasty::Model)]
#[unique(owner, name, filter = "  ")]
#[index(name, filter = "deleted_at IS NULL", filter = "owner > 0")]
struct Token {
    #[key]
    #[auto]
    id: uuid::Uuid,

    owner: i64,
    name: String,
    deleted_at: Option<i64>,
}

fn main() {}
