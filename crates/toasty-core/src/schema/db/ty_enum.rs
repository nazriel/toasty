/// A database enum type with a set of allowed values.
///
/// - On PostgreSQL, this maps to a `CREATE TYPE <name> AS ENUM (...)` named type.
/// - On MySQL, this maps to an inline `ENUM('a', 'b', ...)` column type.
/// - On SQLite, this maps to `TEXT` with a `CHECK` constraint.
/// - On DynamoDB, this is stored as a plain string attribute.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TypeEnum {
    /// The database schema (namespace) a PostgreSQL enum type lives in, when it
    /// is not found through the connection's `search_path`. Only drivers with
    /// [`Capability::schema_qualified_names`](crate::driver::Capability::schema_qualified_names)
    /// accept an enum type with a schema.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub schema: Option<String>,
    /// The type name used by PostgreSQL (`CREATE TYPE <name> AS ENUM`).
    /// `None` for MySQL (inline) and SQLite (CHECK constraint).
    pub name: Option<String>,
    /// Allowed values in declaration order.
    pub variants: Vec<EnumVariant>,
}

/// A single variant in a database enum type.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnumVariant {
    /// The string label for this variant (e.g. `'low'`).
    pub name: String,
}
