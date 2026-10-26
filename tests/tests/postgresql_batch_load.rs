#![cfg(feature = "postgresql")]

//! PostgreSQL batch loads an include with `fk = ANY($1)`, the parent keys
//! bound as one array parameter: one statement text for any number of
//! parents, which the planner serves from the index on the foreign key.
//!
//! The test reads the statement the engine runs from the `toasty::query`
//! event.

use std::sync::{Arc, Mutex};

use toasty_driver_postgresql::PostgreSQL;
use tokio_postgres::NoTls;
use tracing_subscriber::{Layer, layer::SubscriberExt, registry::LookupSpan};

/// The prefix of this test's tables, dropped before it runs.
const TABLE_PREFIX: &str = "toasty_batch_load_";

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

fn url() -> String {
    std::env::var("TOASTY_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/toasty_test".to_string())
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
async fn include_binds_the_parent_keys_as_one_array() {
    let (client, connection) = tokio_postgres::connect(&url(), NoTls)
        .await
        .expect("Failed to connect to PostgreSQL");
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });
    client
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {TABLE_PREFIX}todos, {TABLE_PREFIX}users CASCADE;"
        ))
        .await
        .unwrap();

    let layer = StatementLayer::default();
    let statements = layer.statements.clone();
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    let mut builder = toasty::Db::builder();
    builder.models(toasty::models!(User, Todo));
    builder.table_name_prefix(TABLE_PREFIX);
    let mut db = builder
        .build(PostgreSQL::new(url()).expect("Failed to create PostgreSQL driver"))
        .await
        .unwrap();
    db.push_schema().await.unwrap();

    for (name, titles) in [
        ("Alice", &["a1", "a2"][..]),
        ("Bob", &["b1"]),
        ("Carol", &[]),
    ] {
        let user = User::create().name(name).exec(&mut db).await.unwrap();
        for title in titles {
            user.todos()
                .create()
                .title(*title)
                .exec(&mut db)
                .await
                .unwrap();
        }
    }

    statements.lock().unwrap().clear();
    let users: Vec<User> = User::all()
        .include(User::fields().todos())
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(users.len(), 3);
    let todos: usize = users.iter().map(|user| user.todos.get().len()).sum();
    assert_eq!(todos, 3);

    let statements = statements.lock().unwrap().clone();
    let batch_load: Vec<&String> = statements
        .iter()
        .filter(|statement| statement.contains(&format!(r#"FROM "{TABLE_PREFIX}todos""#)))
        .collect();

    // One statement for the three parents, whose filter is the array
    // membership test and whose only parameter is the array of keys.
    assert_eq!(batch_load.len(), 1, "statements={statements:#?}");
    let batch_load = batch_load[0];
    assert!(
        batch_load.ends_with(r#"WHERE tbl_0_0."user_id" = ANY($1);"#),
        "statement={batch_load}"
    );
    assert!(!batch_load.contains("$2"), "statement={batch_load}");
    assert!(!batch_load.contains("VALUES"), "statement={batch_load}");
}
