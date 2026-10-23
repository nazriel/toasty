#![allow(dead_code)]

// One variant per associated type of every trait the derives implement for the
// user's type: `Field`, `Load`, `Model`, `ViaTarget`, `EmbedCreate`, `Scope`,
// `IntoStatement`, `NewtypeOf`.
#[derive(Debug, PartialEq, toasty::Embed)]
enum Names {
    ExprTarget,
    Path,
    ListPath,
    Update,
    Inner,
    Output,
    Create,
    Query,
    UpdateQuery,
    PrimaryKey,
    ManyField,
    OneField,
    Item,
    Returning,
    Model,
}

// A unit-only enum with the two variants of the original report.
#[derive(Debug, PartialEq, toasty::Embed)]
enum Action {
    Create,
    Update,
}

// A data-carrying enum with variants named like associated types.
#[derive(Debug, PartialEq, toasty::Embed)]
enum Locator {
    Path { value: String },
    Query { text: String },
    Inner,
}

#[derive(Debug, toasty::Model)]
struct Entry {
    #[key]
    id: i64,

    names: Names,
    action: Action,
    locator: Locator,
}

fn main() {
    let _ = Entry::filter(Entry::fields().names().eq(Names::Update));
    let _ = Entry::filter(Entry::fields().names().eq(Names::Path));
    let _ = Entry::filter(Entry::fields().action().eq(Action::Create));
    let _ = Entry::filter(Entry::fields().action().is_update());
    let _ = Entry::filter(Entry::fields().locator().is_path());

    let _ = Entry::create()
        .names(Names::ListPath)
        .action(Action::Update)
        .locator(Locator::Path {
            value: "a".to_string(),
        });

    let _ = Entry::filter_by_id(1).update().action(Action::Create);
}
