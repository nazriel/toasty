#![cfg(feature = "sqlite")]

//! SQLite serves the statement that batch loads a has-many include from the
//! index on the foreign key, not from a scan of the related table.
//!
//! The test reads the statement the engine runs from the `toasty::query`
//! event and asks SQLite for its plan with `EXPLAIN QUERY PLAN`.

use std::sync::{Arc, Mutex};

use toasty_core::stmt::Value;
use tracing_subscriber::{Layer, layer::SubscriberExt, registry::LookupSpan};

#[derive(Debug, toasty::Model)]
struct User {
    #[key]
    #[auto]
    id: u64,

    name: String,

    #[has_many]
    todos: toasty::Deferred<Vec<Todo>>,
}

#[derive(Debug, toasty::Model)]
struct Todo {
    #[key]
    #[auto]
    id: u64,

    #[index]
    user_id: u64,

    #[belongs_to(key = user_id, references = id)]
    user: toasty::Deferred<User>,

    title: String,
}

/// Collects the `db.statement` field of every `toasty::query` event.
#[derive(Clone, Default)]
struct StatementLayer {
    statements: Arc<Mutex<Vec<String>>>,
}

#[derive(Default)]
struct StatementVisitor(Option<String>);

impl tracing::field::Visit for StatementVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "db.statement" {
            self.0 = Some(format!("{value:?}"));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "db.statement" {
            self.0 = Some(value.to_string());
        }
    }
}

impl<S> Layer<S> for StatementLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    // Tests run in parallel, each with its own thread-local subscriber: keep
    // the per-callsite interest from being cached against another test's.
    fn register_callsite(
        &self,
        _metadata: &'static tracing::Metadata<'static>,
    ) -> tracing::subscriber::Interest {
        tracing::subscriber::Interest::sometimes()
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() != "toasty::query" {
            return;
        }
        let mut visitor = StatementVisitor::default();
        event.record(&mut visitor);
        if let Some(statement) = visitor.0 {
            self.statements.lock().unwrap().push(statement);
        }
    }
}

#[tokio::test]
async fn has_many_include_searches_the_foreign_key_index() {
    let layer = StatementLayer::default();
    let statements = layer.statements.clone();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    let mut builder = toasty::Db::builder();
    builder.models(toasty::models!(User, Todo));
    let mut db = builder
        .build(toasty_driver_sqlite::Sqlite::in_memory())
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    let user = User::create().name("Alice").exec(&mut db).await.unwrap();
    for title in ["one", "two"] {
        user.todos()
            .create()
            .title(title)
            .exec(&mut db)
            .await
            .unwrap();
    }

    statements.lock().unwrap().clear();
    let user = User::filter_by_id(user.id)
        .include(User::fields().todos())
        .get(&mut db)
        .await
        .unwrap();
    assert_eq!(user.todos.get().len(), 2);

    let batch_load = statements
        .lock()
        .unwrap()
        .iter()
        .find(|statement| statement.contains(r#"FROM "todos""#))
        .cloned()
        .expect("the include ran a statement over the todos");

    let rows = toasty::sql::query(format!("EXPLAIN QUERY PLAN {batch_load}"))
        .bind(user.id)
        .exec(&mut db)
        .await
        .unwrap();

    // Each row is `(id, parent, notused, detail)`. The rows under parent 0
    // are the outer query's loops; the subquery that lists the keys sits
    // below them.
    let plan: Vec<(i64, String)> = rows
        .into_iter()
        .map(|row| {
            let Value::Record(row) = row else {
                panic!("expected a record; row={row:?}");
            };
            let (Value::I64(parent), Value::String(detail)) = (&row[1], &row[3]) else {
                panic!("expected the plan's parent and detail; row={row:?}");
            };
            (*parent, detail.clone())
        })
        .collect();

    let outer: Vec<&str> = plan
        .iter()
        .filter(|(parent, _)| *parent == 0)
        .map(|(_, detail)| detail.as_str())
        .collect();

    assert!(
        outer
            .iter()
            .any(|detail| detail.contains("USING INDEX index_todos_by_user_id (user_id=?)")),
        "batch load does not search the foreign key index; statement={batch_load}; plan={plan:#?}"
    );
    assert!(
        !outer.iter().any(|detail| detail.starts_with("SCAN")),
        "batch load scans the todos; statement={batch_load}; plan={plan:#?}"
    );
}
