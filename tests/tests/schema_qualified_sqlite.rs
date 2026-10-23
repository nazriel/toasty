#![cfg(feature = "sqlite")]

//! SQLite has no schemas toasty manages (`schema.table` names a table of an
//! attached database), so a model that names one is refused when the `Db` is
//! built. A dot inside a plain table name is still part of the name.

async fn build(models: toasty::schema::app::ModelSet) -> toasty::Result<toasty::Db> {
    toasty::Db::builder()
        .models(models)
        .connect("sqlite::memory:")
        .await
}

#[tokio::test]
async fn schema_qualified_table_is_unsupported() {
    #[derive(Debug, toasty::Model)]
    #[table(schema = "inventory", name = "items")]
    struct Item {
        #[key]
        id: i64,
    }

    let err = build(toasty::models!(Item)).await.unwrap_err();
    assert!(err.is_unsupported_feature(), "{err}");
    assert!(
        err.to_string()
            .contains("schema-qualified table `inventory.items`"),
        "{err}"
    );
}

#[tokio::test]
async fn schema_qualified_enum_type_is_unsupported() {
    #[derive(Debug, toasty::Embed)]
    #[column(type = enum(schema = "inventory", name = "item_status"))]
    enum Status {
        New,
        Sold,
    }

    #[derive(Debug, toasty::Model)]
    struct Item {
        #[key]
        id: i64,
        status: Status,
    }

    let err = build(toasty::models!(Item)).await.unwrap_err();
    assert!(err.is_unsupported_feature(), "{err}");
    assert!(
        err.to_string()
            .contains("schema-qualified enum type `inventory.item_status`"),
        "{err}"
    );
}

#[tokio::test]
async fn dot_in_a_table_name_is_not_a_schema() {
    #[derive(Debug, toasty::Model)]
    #[table = "legacy.items"]
    struct Item {
        #[key]
        id: i64,
    }

    let db = build(toasty::models!(Item)).await.unwrap();
    let table = &db.schema().db.tables[0];
    assert_eq!(table.schema, None);
    assert_eq!(table.name, "legacy.items");
}
