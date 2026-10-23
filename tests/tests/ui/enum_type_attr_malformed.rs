#[derive(toasty::Embed)]
#[column(type = enum(schema = "inventory"))]
enum SchemaWithoutName {
    New,
}

#[derive(toasty::Embed)]
#[column(type = enum(schema = "inventory", label = "item_status"))]
enum UnknownKey {
    New,
}

#[derive(toasty::Embed)]
#[column(type = enum(schema = "", name = "item_status"))]
enum EmptySchema {
    New,
}

#[derive(toasty::Embed)]
#[column(type = enum(schema = "inventory", name = ""))]
enum EmptyName {
    New,
}

#[derive(toasty::Embed)]
#[column(type = enum(name = "item_status", name = "stock_status"))]
enum DuplicateName {
    New,
}

fn main() {}
