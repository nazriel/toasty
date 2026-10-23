#![cfg(feature = "postgresql")]

//! The PostgreSQL driver answers an error, never a panic, where the model and
//! the database disagree. Each test creates its own tables and types with raw
//! SQL, then queries them through models that declare something else.

use toasty::stmt::Value;
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
        .build(PostgreSQL::new(&url()).expect("Failed to create PostgreSQL driver"))
        .await
        .expect("Failed to build Db")
}

#[tokio::test]
async fn enum_type_missing_from_the_database() {
    #[derive(Debug, PartialEq, toasty::Embed)]
    #[column(type = enum("toasty_mismatch_absent_status"))]
    enum Status {
        Open,
        Closed,
    }

    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_enum_items"]
    struct Item {
        #[key]
        id: i64,
        status: Status,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_enum_items;
             DROP TYPE IF EXISTS toasty_mismatch_absent_status;
             CREATE TABLE toasty_mismatch_enum_items (id bigint PRIMARY KEY, status text);",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Item)).await;

    // A statement that binds no value of the enum type never looks it up, so
    // the first query to fail is the one that binds one.
    let err = toasty::create!(Item {
        id: 1,
        status: Status::Open
    })
    .exec(&mut db)
    .await
    .unwrap_err();
    assert!(err.is_invalid_schema(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_enum_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn custom_storage_type_cannot_be_bound() {
    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_custom_items"]
    struct Item {
        #[key]
        id: i64,
        #[column(type = "point")]
        position: String,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_custom_items;
             CREATE TABLE toasty_mismatch_custom_items (id bigint PRIMARY KEY, position point);",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Item)).await;

    let err = toasty::create!(Item {
        id: 1,
        position: "(1,2)"
    })
    .exec(&mut db)
    .await
    .unwrap_err();
    assert!(err.is_unsupported_feature(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_custom_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn integer_out_of_range_for_the_field() {
    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_range_items"]
    struct Item {
        #[key]
        id: i64,
        quantity: u8,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_range_items;
             CREATE TABLE toasty_mismatch_range_items (id bigint PRIMARY KEY, quantity smallint NOT NULL);
             INSERT INTO toasty_mismatch_range_items VALUES (1, 300);",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Item)).await;

    let err = Item::all().exec(&mut db).await.unwrap_err();
    assert!(err.is_type_conversion(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_range_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn integer_out_of_range_for_a_signed_field() {
    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_signed_items"]
    struct Item {
        #[key]
        id: i64,
        delta: i8,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_signed_items;
             CREATE TABLE toasty_mismatch_signed_items (id bigint PRIMARY KEY, delta smallint NOT NULL);
             INSERT INTO toasty_mismatch_signed_items VALUES (1, 300);",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Item)).await;

    let err = Item::all().exec(&mut db).await.unwrap_err();
    assert!(err.is_type_conversion(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_signed_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn text_that_is_not_a_uuid() {
    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_uuid_items"]
    struct Item {
        #[key]
        id: i64,
        token: uuid::Uuid,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_uuid_items;
             CREATE TABLE toasty_mismatch_uuid_items (id bigint PRIMARY KEY, token text NOT NULL);
             INSERT INTO toasty_mismatch_uuid_items VALUES (1, 'not-a-uuid');",
        )
        .await
        .unwrap();

    let mut db = build_db(toasty::models!(Item)).await;

    let err = Item::all().exec(&mut db).await.unwrap_err();
    assert!(err.is_type_conversion(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_uuid_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn column_type_that_does_not_match_the_field() {
    #[derive(Debug, toasty::Model)]
    #[table = "toasty_mismatch_type_items"]
    struct Item {
        #[key]
        id: i64,
        label: String,
        tags: String,
    }

    let client = client().await;
    client
        .batch_execute(
            "DROP TABLE IF EXISTS toasty_mismatch_type_items;
             CREATE TABLE toasty_mismatch_type_items (
                 id bigint PRIMARY KEY, label double precision NOT NULL, tags text[] NOT NULL);
             INSERT INTO toasty_mismatch_type_items VALUES (1, 2.5, '{a,b}');",
        )
        .await
        .unwrap();

    // A `double precision` column read as a string, and a `text[]` column read
    // as a scalar, are result mismatches.
    let mut db = build_db(toasty::models!(Item)).await;
    let err = Item::all().exec(&mut db).await.unwrap_err();
    assert!(err.is_invalid_result(), "{err}");

    client
        .batch_execute("DROP TABLE toasty_mismatch_type_items")
        .await
        .unwrap();
}

#[tokio::test]
async fn raw_query_of_a_void_function() {
    #[derive(Debug, toasty::Model)]
    struct Unused {
        #[key]
        id: i64,
    }

    let db = build_db(toasty::models!(Unused)).await;

    // Advisory locks belong to the session, so lock and unlock on one connection.
    let mut conn = db.connection().await.unwrap();
    let key = 0x7461_7374_7931_i64;

    let rows = toasty::sql::query(format!("SELECT pg_advisory_lock({key})"))
        .exec(&mut conn)
        .await
        .unwrap();
    assert_eq!(rows, vec![Value::record_from_vec(vec![Value::Null])]);

    let rows = toasty::sql::query(format!("SELECT pg_advisory_unlock({key})"))
        .exec(&mut conn)
        .await
        .unwrap();
    assert_eq!(rows, vec![Value::record_from_vec(vec![Value::Bool(true)])]);
}

#[tokio::test]
async fn raw_query_of_an_array() {
    #[derive(Debug, toasty::Model)]
    struct Unused {
        #[key]
        id: i64,
    }

    let mut db = build_db(toasty::models!(Unused)).await;

    let rows = toasty::sql::query("SELECT array['a', 'b']")
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![Value::record_from_vec(vec![Value::List(vec![
            Value::from("a"),
            Value::from("b"),
        ])])]
    );

    let rows = toasty::sql::query("SELECT array[1, 2]::int8[], array[1, null]::int4[]")
        .exec(&mut db)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![Value::record_from_vec(vec![
            Value::List(vec![Value::I64(1), Value::I64(2)]),
            Value::List(vec![Value::I32(1), Value::Null]),
        ])]
    );
}

#[tokio::test]
async fn raw_query_of_a_type_that_cannot_be_inferred() {
    #[derive(Debug, toasty::Model)]
    struct Unused {
        #[key]
        id: i64,
    }

    let mut db = build_db(toasty::models!(Unused)).await;

    let err = toasty::sql::query("SELECT point(1, 2)")
        .exec(&mut db)
        .await
        .unwrap_err();
    assert!(err.is_unsupported_feature(), "{err}");
}
