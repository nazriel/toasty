#![allow(dead_code)]

// The schema-qualified forms of `#[table]` and of a native enum type, beside
// the existing ones. Names are taken whole: a dot is part of a name.

#[derive(Debug, toasty::Embed)]
#[column(type = enum(schema = "inventory", name = "item_status"))]
enum ItemStatus {
    New,
    Sold,
}

#[derive(Debug, toasty::Embed)]
#[column(type = enum(name = "item_kind"))]
enum ItemKind {
    Part,
    Tool,
}

#[derive(Debug, toasty::Embed)]
#[column(type = enum(name = "item_grade", schema = "inventory",))]
enum ItemGrade {
    A,
    B,
}

#[derive(Debug, toasty::Model)]
#[table(schema = "inventory", name = "items")]
struct Item {
    #[key]
    id: i64,

    status: ItemStatus,
    kind: ItemKind,
    grade: ItemGrade,
}

#[derive(Debug, toasty::Model)]
#[table(name = "audit_entries")]
struct Entry {
    #[key]
    id: i64,
}

#[derive(Debug, toasty::Model)]
#[table(schema = "audit")]
struct Note {
    #[key]
    id: i64,
}

#[derive(Debug, toasty::Model)]
#[table = "legacy.items"]
struct LegacyItem {
    #[key]
    id: i64,
}

fn main() {}
