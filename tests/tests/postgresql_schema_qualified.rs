#![cfg(feature = "postgresql")]

//! Models over tables and enum types in PostgreSQL schemas that are not on
//! the connection's `search_path`, named with `#[table(schema = ..., name =
//! ...)]` and `#[column(type = enum(schema = ..., name = ...))]`.
//!
//! The first test creates its schemas with raw SQL, as a schema owned by
//! another tool would be, and never calls `push_schema`. The second lets
//! `push_schema` create a table and its index inside a schema the test
//! created. Each test uses its own schema names so the two can run at once.

use toasty_driver_postgresql::PostgreSQL;
use tokio_postgres::NoTls;

fn url() -> String {
    std::env::var("TOASTY_TEST_POSTGRES_URL")
        .unwrap_or_else(|_| "postgresql://localhost:5432/toasty_test".to_string())
}

/// A plain client for the raw SQL that sets a test up.
async fn client() -> tokio_postgres::Client {
    let (client, connection) = tokio_postgres::connect(&url(), NoTls)
        .await
        .expect("Failed to connect to PostgreSQL");

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            eprintln!("connection error: {e}");
        }
    });

    client
}

async fn build_db(models: toasty::schema::app::ModelSet) -> toasty::Db {
    let mut builder = toasty::Db::builder();
    builder.models(models);
    builder
        .build(PostgreSQL::new(url()).expect("Failed to create PostgreSQL driver"))
        .await
        .expect("Failed to build Db")
}

#[tokio::test]
async fn models_over_two_schemas_outside_the_search_path() {
    #[derive(Debug, PartialEq, toasty::Embed)]
    #[column(type = enum(schema = "toasty_sq_crud_inventory", name = "item_status"))]
    enum Status {
        #[column(variant = "new")]
        New,
        #[column(variant = "sold")]
        Sold,
    }

    #[derive(Debug, toasty::Model)]
    #[table(schema = "toasty_sq_crud_inventory", name = "items")]
    struct Item {
        #[key]
        id: i64,
        name: String,
        status: Status,
    }

    #[derive(Debug, toasty::Model)]
    #[table(schema = "toasty_sq_crud_audit", name = "entries")]
    struct Entry {
        #[key]
        id: i64,
        note: String,

        #[index]
        item_id: i64,
        #[belongs_to]
        item: toasty::Deferred<Item>,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS toasty_sq_crud_inventory CASCADE;
             DROP SCHEMA IF EXISTS toasty_sq_crud_audit CASCADE;
             CREATE SCHEMA toasty_sq_crud_inventory;
             CREATE SCHEMA toasty_sq_crud_audit;
             CREATE TYPE toasty_sq_crud_inventory.item_status AS ENUM ('new', 'sold');
             CREATE TABLE toasty_sq_crud_inventory.items (
                 id bigint PRIMARY KEY,
                 name text NOT NULL,
                 status toasty_sq_crud_inventory.item_status NOT NULL
             );
             CREATE TABLE toasty_sq_crud_audit.entries (
                 id bigint PRIMARY KEY,
                 note text NOT NULL,
                 item_id bigint NOT NULL REFERENCES toasty_sq_crud_inventory.items (id)
             );",
        )
        .await
        .unwrap();

    // Neither schema is on the search path, so only the qualified names can
    // reach them.
    let on_search_path: bool = client
        .query_one(
            "SELECT current_schemas(true) && \
                 ARRAY['toasty_sq_crud_inventory', 'toasty_sq_crud_audit']::name[]",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(!on_search_path);

    let mut db = build_db(toasty::models!(Item, Entry)).await;

    // Create
    let lamp = toasty::create!(Item {
        id: 1,
        name: "lamp",
        status: Status::New
    })
    .exec(&mut db)
    .await
    .unwrap();
    toasty::create!(Item {
        id: 2,
        name: "desk",
        status: Status::Sold
    })
    .exec(&mut db)
    .await
    .unwrap();
    toasty::create!(Entry::[
        { id: 10, note: "received", item: &lamp },
        { id: 11, note: "inspected", item: &lamp },
    ])
    .exec(&mut db)
    .await
    .unwrap();

    // Query with a filter on the schema-qualified enum type
    let sold = Item::filter(Item::fields().status().eq(Status::Sold))
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(sold.len(), 1);
    assert_eq!(sold[0].name, "desk");

    // Include across the relation, from one schema into the other
    let entry = Entry::filter_by_id(10)
        .include(Entry::fields().item())
        .get(&mut db)
        .await
        .unwrap();
    assert_eq!(entry.item.get().name, "lamp");
    assert_eq!(entry.item.get().status, Status::New);

    // Update
    let mut lamp = Item::get_by_id(&mut db, &1).await.unwrap();
    lamp.update()
        .status(Status::Sold)
        .exec(&mut db)
        .await
        .unwrap();
    let status: String = client
        .query_one(
            "SELECT status::text FROM toasty_sq_crud_inventory.items WHERE id = 1",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(status, "sold");

    // Delete
    Entry::filter_by_id(11)
        .delete()
        .exec(&mut db)
        .await
        .unwrap();
    let entries: i64 = client
        .query_one("SELECT count(*) FROM toasty_sq_crud_audit.entries", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(entries, 1);

    client
        .batch_execute(
            "DROP SCHEMA toasty_sq_crud_audit CASCADE;
             DROP SCHEMA toasty_sq_crud_inventory CASCADE;",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn push_schema_creates_a_table_and_its_index_inside_a_schema() {
    #[derive(Debug, PartialEq, toasty::Embed)]
    #[column(type = enum(schema = "toasty_sq_push_inventory", name = "part_kind"))]
    enum Kind {
        #[column(variant = "bolt")]
        Bolt,
        #[column(variant = "nut")]
        Nut,
    }

    #[derive(Debug, toasty::Model)]
    #[table(schema = "toasty_sq_push_inventory", name = "parts")]
    struct Part {
        #[key]
        id: i64,
        #[index]
        sku: String,
        kind: Kind,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP SCHEMA IF EXISTS toasty_sq_push_inventory CASCADE;
             CREATE SCHEMA toasty_sq_push_inventory;",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Part)).await;
    db.push_schema().await.unwrap();

    // The table, its index and its enum type are all in the schema.
    let indexes: Vec<String> = client
        .query(
            "SELECT indexname FROM pg_indexes \
             WHERE schemaname = 'toasty_sq_push_inventory' AND tablename = 'parts' \
             ORDER BY indexname",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(indexes, ["index_parts_by_sku", "parts_pkey"]);

    let type_exists: bool = client
        .query_one(
            "SELECT to_regtype('toasty_sq_push_inventory.part_kind') IS NOT NULL",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert!(type_exists);

    toasty::create!(Part {
        id: 1,
        sku: "B-1",
        kind: Kind::Bolt
    })
    .exec(&mut db)
    .await
    .unwrap();
    let part = Part::filter(Part::fields().kind().eq(Kind::Bolt))
        .get(&mut db)
        .await
        .unwrap();
    assert_eq!(part.sku, "B-1");

    client
        .batch_execute("DROP SCHEMA toasty_sq_push_inventory CASCADE")
        .await
        .unwrap();
}
