use toasty_core::{Error, Result, schema::db};
use tokio_postgres::types::Type;

/// Converts a database storage type to a PostgreSQL wire type.
///
/// Fails with `unsupported_feature` for a storage type the driver has no
/// mapping for, such as a `#[column(type = "...")]` custom type.
pub(crate) fn to_postgres_type(ty: &db::Type) -> Result<&'static Type> {
    Ok(match ty {
        db::Type::Boolean => &Type::BOOL,
        db::Type::Float(4) => &Type::FLOAT4,
        db::Type::Float(8) => &Type::FLOAT8,
        db::Type::Integer(1) => &Type::INT2,
        db::Type::Integer(2) => &Type::INT2,
        db::Type::Integer(4) => &Type::INT4,
        db::Type::Integer(8) => &Type::INT8,
        db::Type::UnsignedInteger(1) => &Type::INT2,
        db::Type::UnsignedInteger(2) => &Type::INT4,
        db::Type::UnsignedInteger(4) => &Type::INT8,
        db::Type::UnsignedInteger(8) => &Type::INT8,
        db::Type::Text | db::Type::VarChar(_) => &Type::TEXT,
        db::Type::Uuid => &Type::UUID,
        db::Type::Numeric(_) => &Type::NUMERIC,
        db::Type::Blob | db::Type::Binary(_) => &Type::BYTEA,
        db::Type::Timestamp(_) => &Type::TIMESTAMPTZ,
        db::Type::Date => &Type::DATE,
        db::Type::Time(_) => &Type::TIME,
        db::Type::DateTime(_) => &Type::TIMESTAMP,
        db::Type::Cidr => &Type::CIDR,
        db::Type::Inet => &Type::INET,
        db::Type::MacAddr => &Type::MACADDR,
        db::Type::MacAddr8 => &Type::MACADDR8,
        // Enum types are handled separately via the cached OID map;
        // fall back to TEXT if we reach here (shouldn't happen in practice).
        db::Type::Enum(_) => &Type::TEXT,
        db::Type::List(elem) => array_type_of(to_postgres_type(elem)?)?,
        // `#[document]` columns: `jsonb` for the binary encoding. The text
        // encoding (`#[document(text)]`) is not yet wired up.
        db::Type::Document { binary: true } => &Type::JSONB,
        db::Type::Document { binary: false } => &Type::JSON,
        db::Type::Json => &Type::JSON,
        db::Type::Jsonb => &Type::JSONB,
        _ => {
            return Err(Error::unsupported_feature(format!(
                "the PostgreSQL driver cannot bind the storage type {ty:?}"
            )));
        }
    })
}

/// Returns the PostgreSQL array type whose element type is `elem`.
///
/// Fails with `unsupported_feature` when `elem` has no array type here.
pub(crate) fn array_type_of(elem: &Type) -> Result<&'static Type> {
    Ok(match *elem {
        Type::BOOL => &Type::BOOL_ARRAY,
        Type::INT2 => &Type::INT2_ARRAY,
        Type::INT4 => &Type::INT4_ARRAY,
        Type::INT8 => &Type::INT8_ARRAY,
        Type::FLOAT4 => &Type::FLOAT4_ARRAY,
        Type::FLOAT8 => &Type::FLOAT8_ARRAY,
        Type::TEXT => &Type::TEXT_ARRAY,
        Type::VARCHAR => &Type::VARCHAR_ARRAY,
        Type::BYTEA => &Type::BYTEA_ARRAY,
        Type::UUID => &Type::UUID_ARRAY,
        Type::NUMERIC => &Type::NUMERIC_ARRAY,
        Type::TIMESTAMP => &Type::TIMESTAMP_ARRAY,
        Type::TIMESTAMPTZ => &Type::TIMESTAMPTZ_ARRAY,
        Type::DATE => &Type::DATE_ARRAY,
        Type::TIME => &Type::TIME_ARRAY,
        Type::CIDR => &Type::CIDR_ARRAY,
        Type::INET => &Type::INET_ARRAY,
        Type::MACADDR => &Type::MACADDR_ARRAY,
        Type::MACADDR8 => &Type::MACADDR8_ARRAY,
        _ => {
            return Err(Error::unsupported_feature(format!(
                "the PostgreSQL driver has no array type for the element type `{elem}`"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_storage_type_is_unsupported() {
        let err = to_postgres_type(&db::Type::Custom("point".into())).unwrap_err();
        assert!(err.is_unsupported_feature());
    }

    #[test]
    fn list_of_custom_storage_type_is_unsupported() {
        let ty = db::Type::List(Box::new(db::Type::Custom("point".into())));
        assert!(to_postgres_type(&ty).unwrap_err().is_unsupported_feature());
    }

    #[test]
    fn element_without_array_type_is_unsupported() {
        assert!(
            array_type_of(&Type::POINT)
                .unwrap_err()
                .is_unsupported_feature()
        );
    }

    #[test]
    fn mapped_types_resolve() {
        assert_eq!(to_postgres_type(&db::Type::Boolean).unwrap(), &Type::BOOL);
        let list = db::Type::List(Box::new(db::Type::Text));
        assert_eq!(to_postgres_type(&list).unwrap(), &Type::TEXT_ARRAY);
    }
}
