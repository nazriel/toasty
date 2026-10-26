//! Relations between models keyed by integers.
//!
//! SQLite stores an auto-increment key narrower than a plain integer column, so
//! an `#[auto] i64` key and the `i64` foreign key referencing it have different
//! storage widths, and only the key is read back through a cast. A `u64` key
//! is read as `u64` at any width.

use crate::prelude::*;
use toasty::Db;
use toasty::stmt::{List, OrderBy, Query};

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    #[auto]
    id: i64,

    #[index]
    name: String,

    #[has_many]
    todos: toasty::Deferred<Vec<Todo>>,
}

#[derive(Debug, toasty::Model)]
struct Todo {
    #[key]
    #[auto]
    id: i64,

    #[index]
    user_id: i64,

    #[belongs_to(key = user_id, references = id)]
    user: toasty::Deferred<User>,

    title: String,

    rank: i64,

    #[has_many]
    steps: toasty::Deferred<Vec<Step>>,
}

#[derive(Debug, toasty::Model)]
struct Step {
    #[key]
    #[auto]
    id: i64,

    #[index]
    todo_id: i64,

    #[belongs_to(key = todo_id, references = id)]
    todo: toasty::Deferred<Todo>,

    #[index]
    text: String,
}

#[derive(Debug, toasty::Model)]
struct Account {
    #[key]
    #[auto]
    id: i32,

    name: String,

    #[has_many]
    entries: toasty::Deferred<Vec<Entry>>,
}

#[derive(Debug, toasty::Model)]
struct Entry {
    #[key]
    #[auto]
    id: i32,

    #[index]
    account_id: i32,

    #[belongs_to(key = account_id, references = id)]
    account: toasty::Deferred<Account>,

    #[index]
    label: String,
}

#[derive(Debug, toasty::Model)]
struct Team {
    #[key]
    #[auto]
    id: u64,

    name: String,

    #[has_many]
    members: toasty::Deferred<Vec<Member>>,
}

#[derive(Debug, toasty::Model)]
struct Member {
    #[key]
    #[auto]
    id: u64,

    #[index]
    team_id: u64,

    #[belongs_to(key = team_id, references = id)]
    team: toasty::Deferred<Team>,

    #[index]
    label: String,

    rank: i64,
}

/// `ann` has two todos, the first with two steps; `bob` has one todo with one
/// step; `cid` has none.
async fn setup_users(test: &mut Test) -> Result<Db> {
    let mut db = test.setup_db(models!(User, Todo, Step)).await;

    for (name, todos) in [
        (
            "ann",
            vec![("a1", 2_i64, vec!["s1", "s2"]), ("a2", 1, vec![])],
        ),
        ("bob", vec![("b1", 3, vec!["s3"])]),
        ("cid", vec![]),
    ] {
        let user = User::create().name(name).exec(&mut db).await?;
        for (title, rank, steps) in todos {
            let todo = Todo::create()
                .user_id(user.id)
                .title(title)
                .rank(rank)
                .exec(&mut db)
                .await?;
            for text in steps {
                Step::create()
                    .todo_id(todo.id)
                    .text(text)
                    .exec(&mut db)
                    .await?;
            }
        }
    }

    Ok(db)
}

fn sorted<'a>(items: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut items: Vec<_> = items.collect();
    items.sort();
    items
}

#[driver_test(requires(sql))]
pub async fn has_many_include_with_i64_keys(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let ann = User::filter_by_name("ann").get(&mut db).await?;
    let ann = User::filter_by_id(ann.id)
        .include(User::fields().todos())
        .get(&mut db)
        .await?;
    assert_eq!(
        sorted(ann.todos.get().iter().map(|todo| todo.title.as_str())),
        ["a1", "a2"]
    );

    let mut users: Vec<User> = User::all()
        .include(User::fields().todos())
        .exec(&mut db)
        .await?;
    users.sort_by(|a, b| a.name.cmp(&b.name));
    let counts: Vec<_> = users.iter().map(|user| user.todos.get().len()).collect();
    assert_eq!(counts, [2, 1, 0]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn belongs_to_include_with_i64_keys(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let mut todos: Vec<Todo> = Todo::all()
        .include(Todo::fields().user())
        .exec(&mut db)
        .await?;
    todos.sort_by(|a, b| a.title.cmp(&b.title));

    let owners: Vec<_> = todos
        .iter()
        .map(|todo| (todo.title.as_str(), todo.user.get().name.as_str()))
        .collect();
    assert_eq!(owners, [("a1", "ann"), ("a2", "ann"), ("b1", "bob")]);

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn nested_include_with_i64_keys(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let users: Vec<User> = User::all()
        .include(User::fields().todos().steps())
        .exec(&mut db)
        .await?;

    let mut steps = vec![];
    for user in &users {
        for todo in user.todos.get() {
            for step in todo.steps.get() {
                steps.push((user.name.as_str(), todo.title.as_str(), step.text.as_str()));
            }
        }
    }
    steps.sort();
    assert_eq!(
        steps,
        [
            ("ann", "a1", "s1"),
            ("ann", "a1", "s2"),
            ("bob", "b1", "s3")
        ]
    );

    // The way back: a step's todo and its user.
    let step = Step::filter_by_text("s3")
        .include(Step::fields().todo().user())
        .get(&mut db)
        .await?;
    assert_eq!(step.todo.get().title, "b1");
    assert_eq!(step.todo.get().user.get().name, "bob");

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn filter_through_a_relation_with_i64_keys(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let users: Vec<User> = User::filter(User::fields().todos().any(Todo::fields().rank().gt(2)))
        .exec(&mut db)
        .await?;
    assert_eq!(
        users
            .iter()
            .map(|user| user.name.as_str())
            .collect::<Vec<_>>(),
        ["bob"]
    );

    // From a todo to the todos of its user.
    let todos: Vec<Todo> = Todo::filter(
        Todo::fields()
            .user()
            .todos()
            .any(Todo::fields().title().eq("a2")),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(
        sorted(todos.iter().map(|todo| todo.title.as_str())),
        ["a1", "a2"]
    );

    // From a step to the todos of its todo's user.
    let steps: Vec<Step> = Step::filter(
        Step::fields()
            .todo()
            .user()
            .todos()
            .any(Todo::fields().title().eq("b1")),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(sorted(steps.iter().map(|step| step.text.as_str())), ["s3"]);

    Ok(())
}

/// The names in `order`, ties broken by name.
async fn names(db: &mut Db, order: impl Into<OrderBy>) -> Result<Vec<String>> {
    let users: Vec<User> = User::all()
        .order_by(order)
        .order_by(User::fields().name().asc())
        .exec(db)
        .await?;
    Ok(users.into_iter().map(|user| user.name).collect())
}

#[driver_test(requires(sql))]
pub async fn order_by_a_related_value_with_i64_keys(test: &mut Test) -> Result<()> {
    let mut db = setup_users(test).await?;

    let rank = |title: &str| {
        Query::<List<Todo>>::all()
            .filter(Todo::fields().title().eq(title))
            .value_for(
                Todo::fields().rank(),
                Todo::fields().user_id(),
                User::fields().id(),
            )
    };

    assert_eq!(
        names(&mut db, rank("a1").desc().nulls_last()).await?,
        ["ann", "bob", "cid"]
    );
    assert_eq!(
        names(&mut db, rank("b1").desc().nulls_last()).await?,
        ["bob", "ann", "cid"]
    );
    assert_eq!(
        names(&mut db, rank("b1").asc().nulls_first()).await?,
        ["ann", "cid", "bob"]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn relations_with_i32_keys(test: &mut Test) -> Result<()> {
    let mut db = test.setup_db(models!(Account, Entry)).await;

    let ann = Account::create()
        .name("ann")
        .entries([Entry::create().label("e1")])
        .entries([Entry::create().label("e2")])
        .exec(&mut db)
        .await?;
    Account::create()
        .name("bob")
        .entries([Entry::create().label("e3")])
        .exec(&mut db)
        .await?;

    let ann = Account::filter_by_id(ann.id)
        .include(Account::fields().entries())
        .get(&mut db)
        .await?;
    assert_eq!(
        sorted(ann.entries.get().iter().map(|entry| entry.label.as_str())),
        ["e1", "e2"]
    );

    let entry = Entry::filter_by_label("e3")
        .include(Entry::fields().account())
        .get(&mut db)
        .await?;
    assert_eq!(entry.account.get().name, "bob");

    let accounts: Vec<Account> = Account::filter(
        Account::fields()
            .entries()
            .any(Entry::fields().label().eq("e3")),
    )
    .exec(&mut db)
    .await?;
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>(),
        ["bob"]
    );

    Ok(())
}

#[driver_test(requires(sql))]
pub async fn relations_with_u64_keys(test: &mut Test) -> Result<()> {
    let mut db = test.setup_db(models!(Team, Member)).await;

    let red = Team::create()
        .name("red")
        .members([Member::create().label("m1").rank(2)])
        .members([Member::create().label("m2").rank(1)])
        .exec(&mut db)
        .await?;
    Team::create()
        .name("blue")
        .members([Member::create().label("m3").rank(3)])
        .exec(&mut db)
        .await?;
    Team::create().name("green").exec(&mut db).await?;

    let red = Team::filter_by_id(red.id)
        .include(Team::fields().members())
        .get(&mut db)
        .await?;
    assert_eq!(
        sorted(red.members.get().iter().map(|member| member.label.as_str())),
        ["m1", "m2"]
    );

    let member = Member::filter_by_label("m3")
        .include(Member::fields().team())
        .get(&mut db)
        .await?;
    assert_eq!(member.team.get().name, "blue");

    let rank = |label: &str| {
        Query::<List<Member>>::all()
            .filter(Member::fields().label().eq(label))
            .value_for(
                Member::fields().rank(),
                Member::fields().team_id(),
                Team::fields().id(),
            )
    };
    let teams: Vec<Team> = Team::all()
        .order_by(rank("m3").desc().nulls_last())
        .order_by(Team::fields().name().asc())
        .exec(&mut db)
        .await?;
    assert_eq!(
        teams
            .iter()
            .map(|team| team.name.as_str())
            .collect::<Vec<_>>(),
        ["blue", "green", "red"]
    );

    Ok(())
}
