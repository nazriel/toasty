use hashbrown::{Equivalent, HashMap, HashSet};

use toasty_core::{Error, Result, schema::db};
use tokio_postgres::{
    Client,
    types::{Kind, Type},
};

use crate::r#type::{array_type_of, to_postgres_type};

/// Lazily caches PostgreSQL OIDs for native enum types — both the base enum
/// (e.g. `status`) and its array form (`_status`). Array OIDs are needed
/// when binding `Value::List` of enum values for `= ANY($1)`.
///
/// A type is cached under its schema and its name: `inventory.status` and an
/// unqualified `status` are two entries.
///
/// Call [`preload`](Self::preload) with all types a statement will use, then
/// [`get`](Self::get) synchronously for each parameter.
#[derive(Debug, Default)]
pub struct OidCache {
    /// Base enum type, indexed by the schema and name the model gives it.
    enum_types: HashMap<TypeKey, Type>,
    /// Array-of-enum type, indexed by the base enum's schema and name.
    enum_array_types: HashMap<TypeKey, Type>,
}

/// A named enum type as the model names it: its schema, when it has one, and
/// its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct TypeKey {
    schema: Option<String>,
    name: String,
}

/// A borrowed [`TypeKey`], for lookups without allocating. Its fields hash
/// exactly as `TypeKey`'s do.
#[derive(Hash)]
struct TypeKeyRef<'a> {
    schema: Option<&'a str>,
    name: &'a str,
}

impl Equivalent<TypeKey> for TypeKeyRef<'_> {
    fn equivalent(&self, key: &TypeKey) -> bool {
        self.schema == key.schema.as_deref() && self.name == key.name
    }
}

impl TypeKey {
    fn as_ref(&self) -> TypeKeyRef<'_> {
        TypeKeyRef {
            schema: self.schema.as_deref(),
            name: &self.name,
        }
    }
}

impl OidCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure all enum types in `types` are cached. Issues at most one query
    /// to `pg_type` for all uncached names. Recurses into `List(...)` so
    /// list-of-enum bind params resolve correctly.
    pub async fn preload<'a>(
        &mut self,
        client: &Client,
        types: impl IntoIterator<Item = &'a db::Type>,
    ) -> Result<()> {
        let mut keys = HashSet::new();
        for ty in types {
            collect_enum_keys(ty, &mut keys);
        }
        let mut uncached: Vec<TypeKey> = keys
            .into_iter()
            .filter(|key| !self.enum_types.contains_key(key))
            .collect();
        // Sorted so the type reported as missing is the same on every run.
        uncached.sort();

        if uncached.is_empty() {
            return Ok(());
        }

        let schemas: Vec<Option<&str>> = uncached.iter().map(|key| key.schema.as_deref()).collect();
        let names: Vec<&str> = uncached.iter().map(|key| key.name.as_str()).collect();

        // `typarray` links each base type to its automatically-created
        // array type, and `pg_enum` gives us the variant labels. Fetching
        // all three in one query keeps preload to a single round-trip and
        // makes the cached `Type` indistinguishable from one tokio_postgres
        // would build itself — so future `Type` equality / `Kind` checks
        // can't trip over an empty variant list.
        //
        // An unqualified name resolves through the session's `search_path`
        // (`to_regtype(quote_ident(name))`), exactly as an unqualified name
        // in a statement would. Matching `pg_type.typname` directly would
        // also match same-named enums in schemas outside the search path —
        // under schema-per-tenant setups the cache could then bind the wrong
        // schema's OID. A schema-qualified name resolves in its own schema
        // only, whether or not that schema is on the search path.
        let rows = client
            .query(
                "SELECT k.schema, k.name, t.typname, t.oid, t.typarray, n.nspname, \
                        array_agg(e.enumlabel ORDER BY e.enumsortorder) \
                 FROM unnest($1::text[], $2::text[]) AS k(schema, name) \
                 JOIN pg_type t ON t.oid = to_regtype(CASE \
                     WHEN k.schema IS NULL THEN quote_ident(k.name) \
                     ELSE format('%I.%I', k.schema, k.name) \
                 END)::oid \
                 JOIN pg_namespace n ON n.oid = t.typnamespace \
                 JOIN pg_enum e ON e.enumtypid = t.oid \
                 GROUP BY k.schema, k.name, t.typname, t.oid, t.typarray, n.nspname",
                &[&schemas, &names],
            )
            .await
            .map_err(toasty_core::Error::driver_operation_failed)?;

        for row in &rows {
            let key = TypeKey {
                schema: row.try_get(0).map_err(Error::driver_operation_failed)?,
                name: row.try_get(1).map_err(Error::driver_operation_failed)?,
            };
            let typname: String = row.try_get(2).map_err(Error::driver_operation_failed)?;
            let oid: u32 = row.try_get(3).map_err(Error::driver_operation_failed)?;
            let array_oid: u32 = row.try_get(4).map_err(Error::driver_operation_failed)?;
            let schema: String = row.try_get(5).map_err(Error::driver_operation_failed)?;
            let variants: Vec<String> = row.try_get(6).map_err(Error::driver_operation_failed)?;

            let enum_type = Type::new(typname.clone(), oid, Kind::Enum(variants), schema.clone());
            let array_type = Type::new(
                format!("_{typname}"),
                array_oid,
                Kind::Array(enum_type.clone()),
                schema,
            );
            self.enum_types.insert(key.clone(), enum_type);
            self.enum_array_types.insert(key, array_type);
        }

        // The query returns a row only for a name that resolves to an enum
        // type, so a requested name without one is not in the database (or,
        // unqualified, is not visible through the session's `search_path`).
        if let Some(missing) = uncached
            .iter()
            .find(|key| !self.enum_types.contains_key(*key))
        {
            return Err(missing_enum_type(missing.as_ref()));
        }

        Ok(())
    }

    /// Look up the PostgreSQL wire type for a `db::Type`. Recurses into
    /// `List(elem)` so list-of-enum and list-of-scalar both resolve to the
    /// correct array OID. Fails with `invalid_schema` if an enum type was not
    /// preloaded, and with `unsupported_feature` for a type with no mapping.
    pub fn get(&self, ty: &db::Type) -> Result<&Type> {
        match ty {
            db::Type::Enum(type_enum) => match type_key_ref(type_enum) {
                Some(key) => self
                    .enum_types
                    .get(&key)
                    .ok_or_else(|| missing_enum_type(key)),
                None => to_postgres_type(ty),
            },
            db::Type::List(elem) => {
                let key = match elem.as_ref() {
                    db::Type::Enum(type_enum) => type_key_ref(type_enum),
                    _ => None,
                };
                match key {
                    Some(key) => self
                        .enum_array_types
                        .get(&key)
                        .ok_or_else(|| missing_enum_type(key)),
                    None => array_type_of(self.get(elem)?),
                }
            }
            _ => to_postgres_type(ty),
        }
    }
}

fn type_key_ref(ty: &db::TypeEnum) -> Option<TypeKeyRef<'_>> {
    Some(TypeKeyRef {
        schema: ty.schema.as_deref(),
        name: ty.name.as_deref()?,
    })
}

fn missing_enum_type(key: TypeKeyRef<'_>) -> Error {
    let name = match key.schema {
        Some(schema) => format!("{schema}.{}", key.name),
        None => key.name.to_string(),
    };
    Error::invalid_schema(format!("enum type `{name}` does not exist in the database"))
}

/// Walk a `db::Type` and collect every named enum's schema and name. Recurses
/// into `List(elem)` so list-of-enum bind params get preloaded.
fn collect_enum_keys(ty: &db::Type, out: &mut HashSet<TypeKey>) {
    match ty {
        db::Type::Enum(te) => {
            if let Some(name) = &te.name {
                out.insert(TypeKey {
                    schema: te.schema.clone(),
                    name: name.clone(),
                });
            }
        }
        db::Type::List(elem) => collect_enum_keys(elem, out),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::OidCache;
    use std::sync::atomic::{AtomicU64, Ordering};
    use toasty_core::schema::db;
    use tokio_postgres::{
        Client, NoTls,
        types::{Kind, Type},
    };

    /// Connect to the PG instance pointed at by `TOASTY_TEST_POSTGRES_URL`.
    /// Returns `None` when the env var is unset so `cargo test` stays green
    /// without a running database; CI sets the variable so the tests run.
    async fn try_connect() -> Option<Client> {
        let url = std::env::var("TOASTY_TEST_POSTGRES_URL").ok()?;
        let (client, conn) = tokio_postgres::connect(&url, NoTls)
            .await
            .expect("PG connection failed");
        tokio::spawn(async move {
            let _ = conn.await;
        });
        Some(client)
    }

    /// Per-process, per-test enum name so concurrent tests don't collide on
    /// the shared PG instance.
    fn enum_name(tag: &str) -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("toasty_oid_{tag}_{}_{n}", std::process::id())
    }

    async fn create_enum(client: &Client, name: &str, variants: &[&str]) {
        client
            .simple_query(&format!("DROP TYPE IF EXISTS {name}"))
            .await
            .unwrap();
        let labels = variants
            .iter()
            .map(|v| format!("'{v}'"))
            .collect::<Vec<_>>()
            .join(", ");
        client
            .simple_query(&format!("CREATE TYPE {name} AS ENUM ({labels})"))
            .await
            .unwrap();
    }

    async fn drop_enum(client: &Client, name: &str) {
        let _ = client
            .simple_query(&format!("DROP TYPE IF EXISTS {name}"))
            .await;
    }

    fn db_enum(name: &str) -> db::Type {
        db::Type::Enum(db::TypeEnum {
            schema: None,
            name: Some(name.to_string()),
            variants: vec![],
        })
    }

    fn db_enum_in(schema: &str, name: &str) -> db::Type {
        db::Type::Enum(db::TypeEnum {
            schema: Some(schema.to_string()),
            name: Some(name.to_string()),
            variants: vec![],
        })
    }

    /// preload populates the cached `Type` with the actual variant labels in
    /// declaration order. Guards against the empty-Vec hack the cache used
    /// to install, which would silently break any future `Type` equality or
    /// kind-introspection check.
    #[tokio::test]
    async fn preload_caches_variants_in_declaration_order() {
        let Some(client) = try_connect().await else {
            return;
        };
        let name = enum_name("variants");
        create_enum(&client, &name, &["pending", "active", "done"]).await;

        let mut cache = OidCache::new();
        cache.preload(&client, [&db_enum(&name)]).await.unwrap();

        assert_eq!(
            cache.get(&db_enum(&name)).unwrap().kind(),
            &Kind::Enum(vec!["pending".into(), "active".into(), "done".into()])
        );

        drop_enum(&client, &name).await;
    }

    /// The cached array type's element is the same `Type` as the cached
    /// scalar — so list-of-enum binds use a `Type` that's
    /// indistinguishable from one tokio_postgres would build itself.
    #[tokio::test]
    async fn preload_caches_array_with_matching_element() {
        let Some(client) = try_connect().await else {
            return;
        };
        let name = enum_name("arr");
        create_enum(&client, &name, &["a", "b"]).await;

        let mut cache = OidCache::new();
        cache.preload(&client, [&db_enum(&name)]).await.unwrap();

        let scalar = cache.get(&db_enum(&name)).unwrap();
        let list = cache
            .get(&db::Type::List(Box::new(db_enum(&name))))
            .unwrap();

        assert_eq!(list.name(), format!("_{name}"));
        match list.kind() {
            Kind::Array(elem) => assert_eq!(elem, scalar),
            other => panic!("expected Kind::Array, got {other:?}"),
        }

        drop_enum(&client, &name).await;
    }

    /// preload recurses into `List(Enum)` so a list-of-enum bind param can
    /// be resolved after a single preload of the list type.
    #[tokio::test]
    async fn preload_recurses_into_list_of_enum() {
        let Some(client) = try_connect().await else {
            return;
        };
        let name = enum_name("recurse");
        create_enum(&client, &name, &["x", "y"]).await;

        let mut cache = OidCache::new();
        let list_ty = db::Type::List(Box::new(db_enum(&name)));
        cache.preload(&client, [&list_ty]).await.unwrap();

        // Both lookups succeed without further preloads.
        assert_eq!(
            cache.get(&db_enum(&name)).unwrap().kind(),
            &Kind::Enum(vec!["x".into(), "y".into()])
        );
        assert_eq!(cache.get(&list_ty).unwrap().name(), format!("_{name}"));

        drop_enum(&client, &name).await;
    }

    /// `List(scalar)` resolves via `array_type_of` with no preload required.
    #[tokio::test]
    async fn list_of_scalar_resolves_without_preload() {
        let cache = OidCache::new();
        assert_eq!(
            cache
                .get(&db::Type::List(Box::new(db::Type::Integer(8))))
                .unwrap(),
            &Type::INT8_ARRAY
        );
        assert_eq!(
            cache
                .get(&db::Type::List(Box::new(db::Type::Text)))
                .unwrap(),
            &Type::TEXT_ARRAY
        );
    }

    /// Preloading the same enum twice neither errors nor mutates the cache.
    #[tokio::test]
    async fn preload_is_idempotent() {
        let Some(client) = try_connect().await else {
            return;
        };
        let name = enum_name("idem");
        create_enum(&client, &name, &["only"]).await;

        let mut cache = OidCache::new();
        cache.preload(&client, [&db_enum(&name)]).await.unwrap();
        let first = cache.get(&db_enum(&name)).unwrap().clone();
        cache.preload(&client, [&db_enum(&name)]).await.unwrap();
        let second = cache.get(&db_enum(&name)).unwrap();
        assert_eq!(&first, second);

        drop_enum(&client, &name).await;
    }

    /// preload resolves enum names through the session `search_path`, so
    /// a same-named enum in another schema cannot shadow the visible one.
    #[tokio::test]
    async fn preload_resolves_via_search_path() {
        let Some(client) = try_connect().await else {
            return;
        };
        let schema_a = enum_name("nsp_a");
        let schema_b = enum_name("nsp_b");
        for (schema, labels) in [(&schema_a, "'a1', 'a2'"), (&schema_b, "'b1'")] {
            client
                .simple_query(&format!("CREATE SCHEMA \"{schema}\""))
                .await
                .unwrap();
            client
                .simple_query(&format!(
                    "CREATE TYPE \"{schema}\".status AS ENUM ({labels})"
                ))
                .await
                .unwrap();
        }

        client
            .simple_query(&format!("SET search_path TO \"{schema_a}\""))
            .await
            .unwrap();
        let mut cache = OidCache::new();
        cache.preload(&client, [&db_enum("status")]).await.unwrap();
        let ty = cache.get(&db_enum("status")).unwrap();
        assert_eq!(ty.kind(), &Kind::Enum(vec!["a1".into(), "a2".into()]));
        assert_eq!(ty.schema(), schema_a);

        client
            .simple_query(&format!("SET search_path TO \"{schema_b}\""))
            .await
            .unwrap();
        let mut cache = OidCache::new();
        cache.preload(&client, [&db_enum("status")]).await.unwrap();
        assert_eq!(
            cache.get(&db_enum("status")).unwrap().kind(),
            &Kind::Enum(vec!["b1".into()])
        );

        for schema in [&schema_a, &schema_b] {
            let _ = client
                .simple_query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
                .await;
        }
    }

    /// Multiple uncached enums are resolved in a single round-trip and each
    /// gets its own variant list.
    #[tokio::test]
    async fn preload_resolves_multiple_enums_in_one_call() {
        let Some(client) = try_connect().await else {
            return;
        };
        let a = enum_name("multi_a");
        let b = enum_name("multi_b");
        create_enum(&client, &a, &["one"]).await;
        create_enum(&client, &b, &["red", "blue"]).await;

        let mut cache = OidCache::new();
        let types = [db_enum(&a), db_enum(&b)];
        cache.preload(&client, types.iter()).await.unwrap();

        assert_eq!(
            cache.get(&db_enum(&a)).unwrap().kind(),
            &Kind::Enum(vec!["one".into()])
        );
        assert_eq!(
            cache.get(&db_enum(&b)).unwrap().kind(),
            &Kind::Enum(vec!["red".into(), "blue".into()])
        );

        drop_enum(&client, &a).await;
        drop_enum(&client, &b).await;
    }

    /// A named enum that is not in the database fails the preload with
    /// `invalid_schema`, and `get` on a name that was never preloaded fails
    /// the same way instead of panicking.
    #[tokio::test]
    async fn missing_enum_type_is_an_invalid_schema_error() {
        let Some(client) = try_connect().await else {
            return;
        };
        let name = enum_name("absent");

        let mut cache = OidCache::new();
        let err = cache.preload(&client, [&db_enum(&name)]).await.unwrap_err();
        assert!(err.is_invalid_schema());

        let err = cache.get(&db_enum(&name)).unwrap_err();
        assert!(err.is_invalid_schema());
        let list = db::Type::List(Box::new(db_enum(&name)));
        assert!(cache.get(&list).unwrap_err().is_invalid_schema());
    }

    /// One missing name among present ones still fails the preload.
    #[tokio::test]
    async fn one_missing_enum_type_fails_the_whole_preload() {
        let Some(client) = try_connect().await else {
            return;
        };
        let present = enum_name("present");
        let absent = enum_name("absent_among");
        create_enum(&client, &present, &["only"]).await;

        let mut cache = OidCache::new();
        let types = [db_enum(&present), db_enum(&absent)];
        let err = cache.preload(&client, types.iter()).await.unwrap_err();
        assert!(err.is_invalid_schema());

        drop_enum(&client, &present).await;
    }

    /// A schema-qualified enum resolves in its own schema, though the schema
    /// is not on the search path, in the same round-trip as an unqualified
    /// one; the qualified and the unqualified name are two cache entries.
    #[tokio::test]
    async fn preload_resolves_a_qualified_enum_outside_the_search_path() {
        let Some(client) = try_connect().await else {
            return;
        };
        let schema = enum_name("nsp_qualified");
        let plain = enum_name("plain");
        client
            .simple_query(&format!(
                "CREATE SCHEMA \"{schema}\"; \
                 CREATE TYPE \"{schema}\".status AS ENUM ('q1', 'q2')"
            ))
            .await
            .unwrap();
        create_enum(&client, &plain, &["p1"]).await;

        let mut cache = OidCache::new();
        let types = [db_enum_in(&schema, "status"), db_enum(&plain)];
        cache.preload(&client, types.iter()).await.unwrap();

        let qualified = cache.get(&db_enum_in(&schema, "status")).unwrap();
        assert_eq!(
            qualified.kind(),
            &Kind::Enum(vec!["q1".into(), "q2".into()])
        );
        assert_eq!(qualified.schema(), schema);
        let list = db::Type::List(Box::new(db_enum_in(&schema, "status")));
        assert_eq!(cache.get(&list).unwrap().name(), "_status");
        assert_eq!(
            cache.get(&db_enum(&plain)).unwrap().kind(),
            &Kind::Enum(vec!["p1".into()])
        );
        // The unqualified `status` was never preloaded.
        assert!(
            cache
                .get(&db_enum("status"))
                .unwrap_err()
                .is_invalid_schema()
        );

        drop_enum(&client, &plain).await;
        let _ = client
            .simple_query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
            .await;
    }

    /// A schema-qualified enum that is not in the database fails the preload,
    /// naming the qualified type.
    #[tokio::test]
    async fn missing_qualified_enum_type_names_its_schema() {
        let Some(client) = try_connect().await else {
            return;
        };
        let schema = enum_name("nsp_absent");

        let mut cache = OidCache::new();
        let err = cache
            .preload(&client, [&db_enum_in(&schema, "status")])
            .await
            .unwrap_err();
        assert!(err.is_invalid_schema());
        assert!(
            err.to_string()
                .contains(&format!("enum type `{schema}.status` does not exist")),
            "{err}"
        );
    }
}
