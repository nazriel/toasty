//! `.include()` loads the related rows of every parent with one statement
//! that filters the related table by the set of parent keys. These tests pin
//! the results and the statement count of the shapes that use it: a has-many
//! include for one parent and for several, a nested include whose keys
//! repeat, and a belongs_to include.

use crate::prelude::*;

use toasty_core::{
    driver::{ExecResponse, Operation},
    stmt::Value,
};

/// Drains the log and returns the queries the driver ran, without the
/// transaction control around them.
fn queries(test: &Test) -> Vec<ExecResponse> {
    let log = test.log();
    let mut queries = vec![];

    while !log.is_empty() {
        let (operation, response) = log.pop();
        if let Operation::QuerySql(_) = operation {
            queries.push(response);
        }
    }

    queries
}

/// The number of rows a query returned.
async fn row_count(response: ExecResponse) -> Result<usize> {
    let Value::List(rows) = response.values.collect_as_value().await? else {
        panic!("expected a list of rows");
    };
    Ok(rows.len())
}

fn sorted<'a>(titles: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut titles: Vec<&str> = titles.into_iter().collect();
    titles.sort();
    titles
}

/// Creates a user with one todo per title.
macro_rules! user_with_todos {
    ($db:expr, $name:expr, [$($title:expr),*]) => {{
        let user = User::create().name($name).exec($db).await?;
        $(user.todos().create().title($title).exec($db).await?;)*
        user
    }};
}

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn has_many_include_for_one_parent(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    let alice = user_with_todos!(&mut db, "Alice", ["a1", "a2", "a3"]);
    user_with_todos!(&mut db, "Bob", ["b1"]);

    test.log().clear();
    let user = User::filter_by_id(alice.id)
        .include(User::fields().todos())
        .get(&mut db)
        .await?;

    let todos = user.todos.get();
    assert_eq!(
        sorted(todos.iter().map(|todo| todo.title.as_str())),
        ["a1", "a2", "a3"]
    );
    for todo in todos {
        assert_eq!(todo.user_id, alice.id);
    }

    // The user, then their todos.
    assert_eq!(queries(test).len(), 2);

    Ok(())
}

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn has_many_include_for_several_parents(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    user_with_todos!(&mut db, "Alice", ["a1", "a2"]);
    user_with_todos!(&mut db, "Bob", ["b1"]);
    user_with_todos!(&mut db, "Carol", []);

    test.log().clear();
    let mut users: Vec<User> = User::all()
        .include(User::fields().todos())
        .exec(&mut db)
        .await?;
    users.sort_by(|a, b| a.name.cmp(&b.name));

    let names: Vec<&str> = users.iter().map(|user| user.name.as_str()).collect();
    assert_eq!(names, ["Alice", "Bob", "Carol"]);
    for (user, expected) in users.iter().zip([&["a1", "a2"][..], &["b1"]]) {
        let titles = sorted(user.todos.get().iter().map(|todo| todo.title.as_str()));
        assert_eq!(titles, expected);
    }
    assert!(users[2].todos.get().is_empty());

    // The users, then the todos of all of them in one statement.
    let mut queries = queries(test);
    assert_eq!(queries.len(), 2);
    assert_eq!(row_count(queries.remove(1)).await?, 3);

    Ok(())
}

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn nested_include_with_repeated_keys(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    let alice = user_with_todos!(&mut db, "Alice", ["a1", "a2", "a3"]);
    user_with_todos!(&mut db, "Bob", ["b1"]);

    test.log().clear();
    let user = User::filter_by_id(alice.id)
        .include(User::fields().todos().user())
        .get(&mut db)
        .await?;

    let todos = user.todos.get();
    assert_eq!(
        sorted(todos.iter().map(|todo| todo.title.as_str())),
        ["a1", "a2", "a3"]
    );
    for todo in todos {
        let owner = todo.user.get();
        assert_eq!(owner.id, alice.id);
        assert_eq!(owner.name, "Alice");
    }

    // The user, the todos, then the todos' users: three todos name the same
    // user, who is read once.
    let mut queries = queries(test);
    assert_eq!(queries.len(), 3);
    assert_eq!(row_count(queries.remove(2)).await?, 1);

    Ok(())
}

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn belongs_to_include_for_several_children(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    let alice = user_with_todos!(&mut db, "Alice", ["a1", "a2"]);
    let bob = user_with_todos!(&mut db, "Bob", ["b1"]);

    test.log().clear();
    let mut todos: Vec<Todo> = Todo::all()
        .include(Todo::fields().user())
        .exec(&mut db)
        .await?;
    todos.sort_by(|a, b| a.title.cmp(&b.title));

    let titles: Vec<&str> = todos.iter().map(|todo| todo.title.as_str()).collect();
    assert_eq!(titles, ["a1", "a2", "b1"]);
    assert_eq!(todos[0].user.get().id, alice.id);
    assert_eq!(todos[1].user.get().id, alice.id);
    assert_eq!(todos[2].user.get().id, bob.id);
    assert_eq!(todos[2].user.get().name, "Bob");

    // The todos, then their users: each user once, though Alice's key
    // appears twice.
    let mut queries = queries(test);
    assert_eq!(queries.len(), 2);
    assert_eq!(row_count(queries.remove(1)).await?, 2);

    Ok(())
}
