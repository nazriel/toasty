#[derive(toasty::Model)]
#[table(schema = "inventory", label = "items")]
struct UnknownKey {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table(name = "")]
struct EmptyName {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table(schema = "", name = "items")]
struct EmptySchema {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table(name = "items", name = "products")]
struct DuplicateName {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table(schema = "inventory", schema = "audit")]
struct DuplicateSchema {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table()]
struct EmptyList {
    #[key]
    id: i64,
}

#[derive(toasty::Model)]
#[table(name = "items")]
#[table = "products"]
struct DuplicateAttribute {
    #[key]
    id: i64,
}

fn main() {}
