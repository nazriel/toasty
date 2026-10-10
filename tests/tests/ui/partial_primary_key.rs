// A primary key covers every row, so `#[key(...)]` takes no `filter`.

#[derive(Debug, toasty::Model)]
#[key(owner, name, filter = "deleted_at IS NULL")]
struct Token {
    owner: i64,
    name: String,
    deleted_at: Option<i64>,
}

fn main() {}
