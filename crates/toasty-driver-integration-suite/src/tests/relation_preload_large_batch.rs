//! An `.include()` binds the key of every parent in the statement that loads
//! the related rows. SQLite allows 32,766 variables in a statement, so a load
//! over more parents runs in chunks; the rows must come back as if it had run
//! in one.

use crate::prelude::*;

/// More parents than SQLite binds variables in one statement.
const PARENTS: usize = 33_000;

/// Rows per `INSERT`, well inside the variable limit.
const INSERT_BATCH: usize = 1_000;

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn has_many_include_above_the_variable_limit(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    let mut user_ids = Vec::with_capacity(PARENTS);
    for start in (0..PARENTS).step_by(INSERT_BATCH) {
        let mut users = User::create_many();
        for i in start..(start + INSERT_BATCH).min(PARENTS) {
            users = users.item(User::create().name(format!("user-{i}")));
        }
        user_ids.extend(users.exec(&mut db).await?.into_iter().map(|user| user.id));
    }

    for batch in user_ids.chunks(INSERT_BATCH) {
        let mut todos = Todo::create_many();
        for id in batch {
            todos = todos.item(Todo::create().user_id(*id).title("todo"));
        }
        todos.exec(&mut db).await?;
    }

    let users: Vec<User> = User::all()
        .include(User::fields().todos())
        .exec(&mut db)
        .await?;

    assert_eq!(users.len(), PARENTS);
    for user in &users {
        let todos = user.todos.get();
        assert_eq!(todos.len(), 1, "user {}", user.name);
        assert_eq!(todos[0].user_id, user.id);
    }

    Ok(())
}

#[driver_test(id(ID), requires(sql), scenario(crate::scenarios::has_many_belongs_to))]
pub async fn belongs_to_include_above_the_variable_limit(test: &mut Test) -> Result<()> {
    let mut db = setup(test).await;

    // Every todo belongs to one of a few users, so the keys of the load repeat
    // across the chunks it is split into.
    let mut user_ids = Vec::new();
    let mut users = User::create_many();
    for i in 0..3 {
        users = users.item(User::create().name(format!("user-{i}")));
    }
    user_ids.extend(users.exec(&mut db).await?.into_iter().map(|user| user.id));

    for start in (0..PARENTS).step_by(INSERT_BATCH) {
        let mut todos = Todo::create_many();
        for i in start..(start + INSERT_BATCH).min(PARENTS) {
            todos = todos.item(Todo::create().user_id(user_ids[i % 3]).title("todo"));
        }
        todos.exec(&mut db).await?;
    }

    let todos: Vec<Todo> = Todo::all()
        .include(Todo::fields().user())
        .exec(&mut db)
        .await?;

    assert_eq!(todos.len(), PARENTS);
    for todo in &todos {
        assert_eq!(todo.user.get().id, todo.user_id);
    }

    Ok(())
}
