use fallible_iterator::FallibleIterator;
use postgres_protocol::types::{ArrayDimension, array_from_sql, array_to_sql};
#[cfg(feature = "net")]
use postgres_protocol::types::{inet_from_sql, inet_to_sql, macaddr_from_sql};
use std::fmt::Display;
#[cfg(feature = "net")]
use toasty_core::stmt::{IpCidr, IpInet, MacAddr6, MacAddr8};
use toasty_core::{
    Error, Result,
    stmt::{self, Value as CoreValue},
};
use tokio_postgres::{
    Column, Row,
    types::{FromSql, IsNull, Kind, ToSql, Type, private::BytesMut, to_sql_checked},
};

/// Wrapper for reading string values from PostgreSQL enum columns.
///
/// The standard `String::FromSql::accepts()` rejects custom enum types.
/// This wrapper accepts `Kind::Enum` types and reads the value as UTF-8 text.
struct EnumString(String);

impl<'a> postgres_types::FromSql<'a> for EnumString {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(EnumString(
            std::str::from_utf8(raw)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Sync + Send>)?
                .to_string(),
        ))
    }

    fn accepts(ty: &Type) -> bool {
        matches!(ty.kind(), Kind::Enum(_))
    }
}

/// Captures the raw wire bytes of a column without interpreting them.
/// Used by the array decode path to hand the column body to
/// [`array_from_sql`] directly, bypassing tokio-postgres's
/// `Vec<Option<T>>` element decoder.
struct RawBytes<'a>(&'a [u8]);

impl<'a> postgres_types::FromSql<'a> for RawBytes<'a> {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(RawBytes(raw))
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }
}

#[derive(Debug)]
pub(crate) struct Value(CoreValue);

impl From<CoreValue> for Value {
    fn from(value: CoreValue) -> Self {
        Self(value)
    }
}

impl Value {
    /// Converts this PostgreSQL driver value into the core Toasty value.
    pub(crate) fn into_inner(self) -> CoreValue {
        self.0
    }

    /// Converts a PostgreSQL value within a row to a Toasty value.
    ///
    /// Fails with `invalid_result` when the column's PostgreSQL type does not
    /// match `expected_ty` or its wire data is malformed, with
    /// `type_conversion` when the value does not fit `expected_ty`, and with
    /// `unsupported_feature` when the column needs a cargo feature the driver
    /// was built without, or a type the driver cannot decode.
    pub(crate) fn from_sql(
        index: usize,
        row: &Row,
        column: &Column,
        expected_ty: &stmt::Type,
    ) -> Result<Self> {
        // Gets the value from the row as Option<T> and return stmt::Value::Null if the Option is
        // None.
        macro_rules! get_or_return_null {
            ($ty:ty) => {{
                match get_opt::<$ty>(row, index, column)? {
                    Some(inner) => inner,
                    None => return Ok(Self(stmt::Value::Null)),
                }
            }};
        }

        // NOTE: unfortunately, the inner representation of the PostgreSQL type enum is not
        // accessible, so we must manually match each type like so.
        let core_value = if column.type_() == &Type::TEXT || column.type_() == &Type::VARCHAR {
            text_to_value(get_or_return_null!(String), expected_ty)?
        } else if column.type_() == &Type::BOOL {
            stmt::Value::Bool(get_or_return_null!(bool))
        } else if column.type_() == &Type::INT2 {
            int2_to_value(get_or_return_null!(i16), column, expected_ty)?
        } else if column.type_() == &Type::INT4 {
            int4_to_value(get_or_return_null!(i32), expected_ty)?
        } else if column.type_() == &Type::INT8 {
            int8_to_value(get_or_return_null!(i64), expected_ty)?
        } else if column.type_() == &Type::UUID {
            let v = get_or_return_null!(uuid::Uuid);
            match expected_ty {
                stmt::Type::Uuid => stmt::Value::Uuid(v),
                stmt::Type::String => stmt::Value::String(v.to_string()),
                _ => stmt::Value::Uuid(v),
            }
        } else if column.type_() == &Type::BYTEA {
            bytea_to_value(get_or_return_null!(Vec<u8>), column, expected_ty)?
        } else if column.type_() == &Type::TIMESTAMPTZ {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Timestamp(get_or_return_null!(jiff::Timestamp))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::TIMESTAMP {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::DateTime(get_or_return_null!(jiff::civil::DateTime))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::DATE {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Date(get_or_return_null!(jiff::civil::Date))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::TIME {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Time(get_or_return_null!(jiff::civil::Time))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::CIDR {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::Cidr(decode_cidr(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::INET {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::Inet(decode_inet(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::MACADDR {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::MacAddr(decode_macaddr(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::MACADDR8 {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::MacAddr8(decode_macaddr8(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::FLOAT4 {
            float4_to_value(get_or_return_null!(f32), column, expected_ty)?
        } else if column.type_() == &Type::FLOAT8 {
            float8_to_value(get_or_return_null!(f64), column, expected_ty)?
        } else if column.type_() == &Type::NUMERIC {
            #[cfg(feature = "rust_decimal")]
            {
                stmt::Value::Decimal(get_or_return_null!(rust_decimal::Decimal))
            }
            #[cfg(not(feature = "rust_decimal"))]
            {
                return Err(feature_required(column.type_(), "rust_decimal"));
            }
        } else if matches!(column.type_().kind(), Kind::Enum(_)) {
            // Native database enum types (CREATE TYPE ... AS ENUM) are read as strings.
            // We use EnumString instead of String because String::FromSql::accepts()
            // rejects custom enum types.
            match get_opt::<EnumString>(row, index, column)? {
                Some(EnumString(v)) => stmt::Value::String(v),
                None => return Ok(Self(stmt::Value::Null)),
            }
        } else if column.type_() == &Type::JSONB || column.type_() == &Type::JSON {
            let raw = match get_opt::<RawBytes<'_>>(row, index, column)? {
                Some(RawBytes(raw)) => raw,
                None => return Ok(Self(stmt::Value::Null)),
            };
            // `jsonb` wire format: a 1-byte version header, then UTF-8 JSON
            // text. `json` is plain UTF-8 text.
            let json_bytes = if column.type_() == &Type::JSONB {
                match raw.split_first() {
                    Some((1, rest)) => rest,
                    _ => {
                        return Err(read_error(column, "unsupported jsonb wire version"));
                    }
                }
            } else {
                raw
            };

            if expected_ty == &stmt::Type::String {
                // `Json<T>` is opaque to the engine. Preserve the serialized
                // JSON so `Json<T>::load` can deserialize it directly into T.
                // `String::from_utf8` reuses this single allocation.
                stmt::Value::String(
                    String::from_utf8(json_bytes.to_vec())
                        .map_err(|e| read_error(column, format!("non-UTF-8 JSON: {e}")))?,
                )
            } else {
                // `#[document]` columns are queryable by the engine, so decode
                // them to their structural `Value::Object` representation.
                toasty_sql::json::from_slice(json_bytes, expected_ty)
                    .map_err(|e| read_error(column, format!("invalid JSON: {e}")))?
            }
        } else if let Kind::Array(elem_pg_ty) = column.type_().kind() {
            // Native array column (e.g. `text[]`, `int8[]`) — decoded
            // directly into `Vec<stmt::Value>` via `array_from_sql`. The
            // element type is taken from `expected_ty.as_list_unwrap()` so
            // the per-element conversion mirrors a column of that scalar
            // type.
            let elem_ty = match expected_ty {
                stmt::Type::List(elem) => elem.as_ref(),
                other => return Err(mismatch(column, other)),
            };
            match read_array_items(index, row, column, elem_pg_ty, elem_ty)? {
                Some(items) => stmt::Value::List(items),
                None => return Ok(Self(stmt::Value::Null)),
            }
        } else {
            return Err(Error::unsupported_feature(format!(
                "the PostgreSQL driver cannot read the type `{}` of column `{}`",
                column.type_(),
                column.name()
            )));
        };

        Ok(Value(core_value))
    }

    /// Converts a PostgreSQL value within a row using PostgreSQL column
    /// metadata as the Toasty value type.
    ///
    /// Fails with `unsupported_feature` for a column type that cannot be
    /// inferred, or that needs a cargo feature the driver was built without.
    pub(crate) fn from_sql_infer(index: usize, row: &Row, column: &Column) -> Result<Self> {
        macro_rules! get_or_return_null {
            ($ty:ty) => {{
                match get_opt::<$ty>(row, index, column)? {
                    Some(inner) => inner,
                    None => return Ok(Self(stmt::Value::Null)),
                }
            }};
        }

        let core_value = if column.type_() == &Type::TEXT || column.type_() == &Type::VARCHAR {
            stmt::Value::String(get_or_return_null!(String))
        } else if column.type_() == &Type::BOOL {
            stmt::Value::Bool(get_or_return_null!(bool))
        } else if column.type_() == &Type::INT2 {
            stmt::Value::I16(get_or_return_null!(i16))
        } else if column.type_() == &Type::INT4 {
            stmt::Value::I32(get_or_return_null!(i32))
        } else if column.type_() == &Type::INT8 {
            stmt::Value::I64(get_or_return_null!(i64))
        } else if column.type_() == &Type::UUID {
            stmt::Value::Uuid(get_or_return_null!(uuid::Uuid))
        } else if column.type_() == &Type::BYTEA {
            stmt::Value::Bytes(get_or_return_null!(Vec<u8>))
        } else if column.type_() == &Type::TIMESTAMPTZ {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Timestamp(get_or_return_null!(jiff::Timestamp))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::TIMESTAMP {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::DateTime(get_or_return_null!(jiff::civil::DateTime))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::DATE {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Date(get_or_return_null!(jiff::civil::Date))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::TIME {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Time(get_or_return_null!(jiff::civil::Time))
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(column.type_(), "jiff"));
            }
        } else if column.type_() == &Type::CIDR {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::Cidr(decode_cidr(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::INET {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::Inet(decode_inet(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::MACADDR {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::MacAddr(decode_macaddr(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::MACADDR8 {
            #[cfg(feature = "net")]
            {
                let raw = get_or_return_null!(RawBytes<'_>).0;
                stmt::Value::MacAddr8(decode_macaddr8(raw, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(column.type_(), "net"));
            }
        } else if column.type_() == &Type::FLOAT4 {
            stmt::Value::F32(get_or_return_null!(f32))
        } else if column.type_() == &Type::FLOAT8 {
            stmt::Value::F64(get_or_return_null!(f64))
        } else if column.type_() == &Type::NUMERIC {
            #[cfg(feature = "rust_decimal")]
            {
                stmt::Value::Decimal(get_or_return_null!(rust_decimal::Decimal))
            }
            #[cfg(not(feature = "rust_decimal"))]
            {
                return Err(feature_required(column.type_(), "rust_decimal"));
            }
        } else if matches!(column.type_().kind(), Kind::Enum(_)) {
            match get_opt::<EnumString>(row, index, column)? {
                Some(EnumString(v)) => stmt::Value::String(v),
                None => return Ok(Self(stmt::Value::Null)),
            }
        } else if column.type_() == &Type::VOID {
            // A function returning `void` (e.g. `pg_advisory_lock`) yields a
            // value with no content.
            stmt::Value::Null
        } else if let Kind::Array(elem_pg_ty) = column.type_().kind() {
            // One-dimensional arrays of the element types inferred above.
            let elem_ty = infer_array_element_ty(elem_pg_ty);
            match read_array_items(index, row, column, elem_pg_ty, &elem_ty)? {
                Some(items) => stmt::Value::List(items),
                None => return Ok(Self(stmt::Value::Null)),
            }
        } else {
            return Err(Error::unsupported_feature(format!(
                "the PostgreSQL driver cannot infer the type `{}` of column `{}`",
                column.type_(),
                column.name()
            )));
        };

        Ok(Value(core_value))
    }
}

/// Reads column `index` as `Option<T>`, answering `invalid_result` when the
/// column's PostgreSQL type is not one `T` accepts.
fn get_opt<'a, T: FromSql<'a>>(row: &'a Row, index: usize, column: &Column) -> Result<Option<T>> {
    row.try_get::<usize, Option<T>>(index)
        .map_err(|e| read_error(column, e))
}

/// An `invalid_result` error for a column whose data cannot be read.
fn read_error(column: &Column, cause: impl Display) -> Error {
    Error::invalid_result(format!(
        "cannot read column `{}` of PostgreSQL type `{}`: {cause}",
        column.name(),
        column.type_()
    ))
}

/// An `invalid_result` error for a column whose PostgreSQL type has no
/// conversion to the type the statement expects.
fn mismatch(column: &Column, expected_ty: &stmt::Type) -> Error {
    Error::invalid_result(format!(
        "column `{}` has PostgreSQL type `{}`, which cannot be read as {expected_ty:?}",
        column.name(),
        column.type_()
    ))
}

/// An `unsupported_feature` error for a PostgreSQL type that is only readable
/// with a cargo feature the driver was built without.
#[allow(dead_code)] // unused when every optional feature is enabled
fn feature_required(pg_ty: &Type, feature: &str) -> Error {
    Error::unsupported_feature(format!(
        "reading the PostgreSQL type `{pg_ty}` requires the `{feature}` feature of toasty-driver-postgresql"
    ))
}

/// The scalar type a raw query's array element of PostgreSQL type `elem_pg_ty`
/// is inferred as. The decoders of the types not listed ignore it.
fn infer_array_element_ty(elem_pg_ty: &Type) -> stmt::Type {
    if elem_pg_ty == &Type::INT2 {
        stmt::Type::I16
    } else if elem_pg_ty == &Type::INT4 {
        stmt::Type::I32
    } else if elem_pg_ty == &Type::INT8 {
        stmt::Type::I64
    } else if elem_pg_ty == &Type::FLOAT4 {
        stmt::Type::F32
    } else if elem_pg_ty == &Type::FLOAT8 {
        stmt::Type::F64
    } else if elem_pg_ty == &Type::UUID {
        stmt::Type::Uuid
    } else if elem_pg_ty == &Type::BYTEA {
        stmt::Type::Bytes
    } else {
        stmt::Type::String
    }
}

// ============================================================================
// Per-primitive conversions
// ----------------------------------------------------------------------------
// These functions translate a single decoded PostgreSQL primitive (the value
// you get from `Row::get` or from an array element) into a `stmt::Value`,
// respecting Toasty's expected element type. Sharing them between the column
// path ([`Value::from_sql`]) and the array path ([`read_array_items`]) keeps
// the two reading paths consistent: a `text[]` element and a `text` column
// decode through the same logic.
// ============================================================================

fn text_to_value(v: String, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::String => stmt::Value::String(v),
        stmt::Type::Uuid => match v.parse() {
            Ok(uuid) => stmt::Value::Uuid(uuid),
            Err(_) => return Err(Error::type_conversion(stmt::Value::String(v), "Uuid")),
        },
        _ => stmt::Value::String(v),
    })
}

fn bytea_to_value(v: Vec<u8>, column: &Column, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::Uuid => match uuid::Uuid::from_slice(&v) {
            Ok(uuid) => stmt::Value::Uuid(uuid),
            Err(_) => return Err(Error::type_conversion(stmt::Value::Bytes(v), "Uuid")),
        },
        stmt::Type::Bytes => stmt::Value::Bytes(v),
        _ => return Err(mismatch(column, expected_ty)),
    })
}

fn int2_to_value(v: i16, column: &Column, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::I8 => match i8::try_from(v) {
            Ok(v) => stmt::Value::I8(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I16(v), "i8")),
        },
        stmt::Type::I16 => stmt::Value::I16(v),
        stmt::Type::U8 => match u8::try_from(v) {
            Ok(v) => stmt::Value::U8(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I16(v), "u8")),
        },
        stmt::Type::U16 => match u16::try_from(v) {
            Ok(v) => stmt::Value::U16(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I16(v), "u16")),
        },
        _ => return Err(mismatch(column, expected_ty)),
    })
}

fn int4_to_value(v: i32, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::I32 => stmt::Value::I32(v),
        stmt::Type::U16 => match u16::try_from(v) {
            Ok(v) => stmt::Value::U16(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I32(v), "u16")),
        },
        stmt::Type::U32 => match u32::try_from(v) {
            Ok(v) => stmt::Value::U32(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I32(v), "u32")),
        },
        _ => stmt::Value::I32(v),
    })
}

fn int8_to_value(v: i64, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::I64 => stmt::Value::I64(v),
        stmt::Type::U32 => match u32::try_from(v) {
            Ok(v) => stmt::Value::U32(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I64(v), "u32")),
        },
        stmt::Type::U64 => match u64::try_from(v) {
            Ok(v) => stmt::Value::U64(v),
            Err(_) => return Err(Error::type_conversion(stmt::Value::I64(v), "u64")),
        },
        _ => stmt::Value::I64(v),
    })
}

fn float4_to_value(v: f32, column: &Column, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::F32 => stmt::Value::F32(v),
        stmt::Type::F64 => stmt::Value::F64(v as f64),
        _ => return Err(mismatch(column, expected_ty)),
    })
}

/// Whether `v` is finite and beyond the range of `f32`. Such a value would
/// become an infinity in a `float4` column.
fn exceeds_f32(v: f64) -> bool {
    v.is_finite() && v.abs() > f32::MAX as f64
}

/// `v` as the `f32` a `float4` column holds. A value that [`exceeds_f32`] is an
/// error; anything in range is rounded to the nearest `f32`.
fn f64_to_f32(v: f64) -> Result<f32> {
    if exceeds_f32(v) {
        return Err(Error::type_conversion(stmt::Value::F64(v), "f32"));
    }
    Ok(v as f32)
}

fn float8_to_value(v: f64, column: &Column, expected_ty: &stmt::Type) -> Result<stmt::Value> {
    Ok(match expected_ty {
        stmt::Type::F32 => stmt::Value::F32(f64_to_f32(v)?),
        stmt::Type::F64 => stmt::Value::F64(v),
        _ => return Err(mismatch(column, expected_ty)),
    })
}

/// Decode a PostgreSQL array column into a list of Toasty values. Returns
/// `None` for SQL NULL. The column body is parsed with [`array_from_sql`]
/// and each element is decoded straight into a `stmt::Value` via the
/// per-primitive helpers above — the same ones used by [`Value::from_sql`]
/// for scalar columns. No intermediate `Vec<Option<T>>` is allocated.
fn read_array_items(
    index: usize,
    row: &Row,
    column: &Column,
    elem_pg_ty: &Type,
    elem_ty: &stmt::Type,
) -> Result<Option<Vec<stmt::Value>>> {
    let Some(RawBytes(raw)) = get_opt::<RawBytes<'_>>(row, index, column)? else {
        return Ok(None);
    };
    let array = array_from_sql(raw)
        .map_err(|e| read_error(column, format!("invalid array wire format: {e}")))?;

    let ndims = array
        .dimensions()
        .count()
        .map_err(|e| read_error(column, format!("invalid array dimensions header: {e}")))?;
    if ndims > 1 {
        return Err(read_error(
            column,
            format!(
                "multi-dimensional arrays are not supported (got {ndims} dimensions). \
                 See https://github.com/tokio-rs/toasty/issues/870"
            ),
        ));
    }

    let mut values = array.values();
    let (cap, _) = values.size_hint();
    let mut out = Vec::with_capacity(cap);
    while let Some(elem) = values
        .next()
        .map_err(|e| read_error(column, format!("invalid array element framing: {e}")))?
    {
        out.push(match elem {
            None => stmt::Value::Null,
            Some(bytes) => decode_array_element(column, elem_pg_ty, bytes, elem_ty)?,
        });
    }
    Ok(Some(out))
}

/// Decodes a value of PostgreSQL type `T` from the wire bytes of an array
/// element of `column`.
fn decode_element<'a, T: FromSql<'a>>(
    column: &Column,
    elem_pg_ty: &Type,
    bytes: &'a [u8],
) -> Result<T> {
    T::from_sql(elem_pg_ty, bytes)
        .map_err(|e| read_error(column, format!("invalid `{elem_pg_ty}` array element: {e}")))
}

/// Decode a single PostgreSQL array element from its raw wire bytes and
/// project it into the Toasty value space using `elem_ty`. Reuses the
/// scalar-column `_to_value` helpers so a `text[]` element and a `text`
/// column decode through identical logic.
fn decode_array_element(
    column: &Column,
    elem_pg_ty: &Type,
    bytes: &[u8],
    elem_ty: &stmt::Type,
) -> Result<stmt::Value> {
    Ok(
        if elem_pg_ty == &Type::TEXT || elem_pg_ty == &Type::VARCHAR {
            text_to_value(decode_element(column, elem_pg_ty, bytes)?, elem_ty)?
        } else if elem_pg_ty == &Type::BOOL {
            stmt::Value::Bool(decode_element(column, elem_pg_ty, bytes)?)
        } else if elem_pg_ty == &Type::INT2 {
            int2_to_value(decode_element(column, elem_pg_ty, bytes)?, column, elem_ty)?
        } else if elem_pg_ty == &Type::INT4 {
            int4_to_value(decode_element(column, elem_pg_ty, bytes)?, elem_ty)?
        } else if elem_pg_ty == &Type::INT8 {
            int8_to_value(decode_element(column, elem_pg_ty, bytes)?, elem_ty)?
        } else if elem_pg_ty == &Type::FLOAT4 {
            float4_to_value(decode_element(column, elem_pg_ty, bytes)?, column, elem_ty)?
        } else if elem_pg_ty == &Type::FLOAT8 {
            float8_to_value(decode_element(column, elem_pg_ty, bytes)?, column, elem_ty)?
        } else if elem_pg_ty == &Type::BYTEA {
            bytea_to_value(decode_element(column, elem_pg_ty, bytes)?, column, elem_ty)?
        } else if elem_pg_ty == &Type::NUMERIC {
            #[cfg(feature = "rust_decimal")]
            {
                stmt::Value::Decimal(decode_element::<rust_decimal::Decimal>(
                    column, elem_pg_ty, bytes,
                )?)
            }
            #[cfg(not(feature = "rust_decimal"))]
            {
                return Err(feature_required(elem_pg_ty, "rust_decimal"));
            }
        } else if elem_pg_ty == &Type::UUID {
            stmt::Value::Uuid(decode_element(column, elem_pg_ty, bytes)?)
        } else if elem_pg_ty == &Type::TIMESTAMPTZ {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Timestamp(decode_element::<jiff::Timestamp>(
                    column, elem_pg_ty, bytes,
                )?)
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(elem_pg_ty, "jiff"));
            }
        } else if elem_pg_ty == &Type::TIMESTAMP {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::DateTime(decode_element::<jiff::civil::DateTime>(
                    column, elem_pg_ty, bytes,
                )?)
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(elem_pg_ty, "jiff"));
            }
        } else if elem_pg_ty == &Type::DATE {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Date(decode_element::<jiff::civil::Date>(
                    column, elem_pg_ty, bytes,
                )?)
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(elem_pg_ty, "jiff"));
            }
        } else if elem_pg_ty == &Type::TIME {
            #[cfg(feature = "jiff")]
            {
                stmt::Value::Time(decode_element::<jiff::civil::Time>(
                    column, elem_pg_ty, bytes,
                )?)
            }
            #[cfg(not(feature = "jiff"))]
            {
                return Err(feature_required(elem_pg_ty, "jiff"));
            }
        } else if elem_pg_ty == &Type::CIDR {
            #[cfg(feature = "net")]
            {
                stmt::Value::Cidr(decode_cidr(bytes, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(elem_pg_ty, "net"));
            }
        } else if elem_pg_ty == &Type::INET {
            #[cfg(feature = "net")]
            {
                stmt::Value::Inet(decode_inet(bytes, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(elem_pg_ty, "net"));
            }
        } else if elem_pg_ty == &Type::MACADDR {
            #[cfg(feature = "net")]
            {
                stmt::Value::MacAddr(decode_macaddr(bytes, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(elem_pg_ty, "net"));
            }
        } else if elem_pg_ty == &Type::MACADDR8 {
            #[cfg(feature = "net")]
            {
                stmt::Value::MacAddr8(decode_macaddr8(bytes, column)?)
            }
            #[cfg(not(feature = "net"))]
            {
                return Err(feature_required(elem_pg_ty, "net"));
            }
        } else if matches!(elem_pg_ty.kind(), Kind::Enum(_)) {
            // Enum labels are plain UTF-8; `EnumString` accepts `Kind::Enum` where
            // `String` won't.
            let EnumString(v) = decode_element(column, elem_pg_ty, bytes)?;
            text_to_value(v, elem_ty)?
        } else {
            return Err(Error::unsupported_feature(format!(
                "the PostgreSQL driver cannot read array elements of type `{elem_pg_ty}` (column `{}`)",
                column.name()
            )));
        },
    )
}

impl ToSql for Value {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> std::result::Result<IsNull, Box<dyn std::error::Error + Sync + Send>>
    where
        Self: Sized,
    {
        value_to_sql(&self.0, ty, out)
    }

    fn accepts(ty: &Type) -> bool {
        matches!(
            *ty,
            Type::BOOL
                | Type::INT2
                | Type::INT4
                | Type::INT8
                | Type::TEXT
                | Type::FLOAT4
                | Type::FLOAT8
                | Type::VARCHAR
                | Type::BYTEA
                | Type::UUID
                | Type::NUMERIC
                | Type::TIMESTAMP
                | Type::TIMESTAMPTZ
                | Type::DATE
                | Type::TIME
                | Type::JSONB
                | Type::JSON
                | Type::CIDR
                | Type::INET
                | Type::MACADDR
                | Type::MACADDR8
        ) || matches!(ty.kind(), Kind::Enum(_) | Kind::Array(_))
    }
    to_sql_checked!();
}

/// Free-fn form of `Value::to_sql` so the array-element closure can call it
/// with a `&CoreValue` borrowed from the input slice — no per-element clone
/// or wrapper construction.
fn value_to_sql(
    value: &CoreValue,
    ty: &Type,
    out: &mut BytesMut,
) -> std::result::Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
    match (value, ty) {
        (stmt::Value::Bool(value), _) => value.to_sql(ty, out),
        (stmt::Value::I8(value), &Type::INT2) => (*value as i16).to_sql(ty, out),
        (stmt::Value::I8(value), &Type::INT4) => (*value as i32).to_sql(ty, out),
        (stmt::Value::I8(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::I16(value), &Type::INT2) => value.to_sql(ty, out),
        (stmt::Value::I16(value), &Type::INT4) => (*value as i32).to_sql(ty, out),
        (stmt::Value::I16(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::I32(value), &Type::INT4) => value.to_sql(ty, out),
        (stmt::Value::I32(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::I64(value), &Type::INT4) => match i32::try_from(*value) {
            Ok(value) => value.to_sql(ty, out),
            Err(_) => Err(format!(
                "i64 value {value} is out of range for the PostgreSQL type `{ty}`"
            )
            .into()),
        },
        (stmt::Value::I64(value), &Type::INT8) => value.to_sql(ty, out),
        (stmt::Value::U8(value), &Type::INT2) => (*value as i16).to_sql(ty, out),
        (stmt::Value::U8(value), &Type::INT4) => (*value as i32).to_sql(ty, out),
        (stmt::Value::U8(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::U16(value), &Type::INT4) => (*value as i32).to_sql(ty, out),
        (stmt::Value::U16(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::U32(value), &Type::INT8) => (*value as i64).to_sql(ty, out),
        (stmt::Value::U64(value), &Type::INT8) => {
            if *value > i64::MAX as u64 {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "u64 value {} exceeds i64::MAX ({}), cannot store in PostgreSQL BIGINT",
                        value,
                        i64::MAX
                    ),
                )));
            }
            (*value as i64).to_sql(ty, out)
        }
        (stmt::Value::F32(value), &Type::FLOAT4) => value.to_sql(ty, out),
        (stmt::Value::F32(value), &Type::FLOAT8) => (*value as f64).to_sql(ty, out),
        (stmt::Value::F64(value), &Type::FLOAT4) if exceeds_f32(*value) => {
            Err(format!("f64 value {value} is out of range for the PostgreSQL type `{ty}`").into())
        }
        (stmt::Value::F64(value), &Type::FLOAT4) => (*value as f32).to_sql(ty, out),
        (stmt::Value::F64(value), &Type::FLOAT8) => value.to_sql(ty, out),
        (stmt::Value::Null, _) => Ok(IsNull::Yes),
        // `Json<T>` values are already serialized once by serde. Bind their
        // bytes directly instead of decoding and re-encoding a JSON tree.
        (stmt::Value::String(value), &Type::JSONB) => {
            out.extend_from_slice(&[1]);
            out.extend_from_slice(value.as_bytes());
            Ok(IsNull::No)
        }
        (stmt::Value::String(value), &Type::JSON) => {
            out.extend_from_slice(value.as_bytes());
            Ok(IsNull::No)
        }
        // PG enums are wire-encoded as plain UTF-8 text. `String::ToSql::accepts`
        // rejects `Kind::Enum`, so write the bytes directly.
        (stmt::Value::String(value), _) if matches!(ty.kind(), Kind::Enum(_)) => {
            out.extend_from_slice(value.as_bytes());
            Ok(IsNull::No)
        }
        (stmt::Value::String(value), _) => value.to_sql(ty, out),
        (stmt::Value::Bytes(value), &Type::BYTEA) => value.to_sql(ty, out),
        (stmt::Value::Uuid(value), &Type::UUID) => value.to_sql(ty, out),
        #[cfg(feature = "rust_decimal")]
        (stmt::Value::Decimal(value), _) => value.to_sql(ty, out),
        #[cfg(feature = "jiff")]
        (stmt::Value::Timestamp(value), _) => value.to_sql(ty, out),
        #[cfg(feature = "jiff")]
        (stmt::Value::Date(value), _) => value.to_sql(ty, out),
        #[cfg(feature = "jiff")]
        (stmt::Value::Time(value), _) => value.to_sql(ty, out),
        #[cfg(feature = "jiff")]
        (stmt::Value::DateTime(value), _) => value.to_sql(ty, out),
        #[cfg(feature = "net")]
        (stmt::Value::Cidr(value), &Type::CIDR) => {
            inet_to_sql(value.first_address(), value.network_length(), out);
            Ok(IsNull::No)
        }
        #[cfg(feature = "net")]
        (stmt::Value::Inet(value), &Type::INET) => {
            inet_to_sql(value.address(), value.network_length(), out);
            Ok(IsNull::No)
        }
        #[cfg(feature = "net")]
        (stmt::Value::MacAddr(value), &Type::MACADDR) => {
            out.extend_from_slice(value.as_bytes());
            Ok(IsNull::No)
        }
        #[cfg(feature = "net")]
        (stmt::Value::MacAddr8(value), &Type::MACADDR8) => {
            out.extend_from_slice(value.as_bytes());
            Ok(IsNull::No)
        }
        // `#[document]` columns: serialize the structural value (a
        // `Value::List` of `Value::Object`s) to JSON and bind it. The engine
        // has already converted positional records to named objects, so this
        // is a plain structural encode.
        (value, &Type::JSONB) => {
            let bytes = toasty_sql::json::to_vec(value)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Sync + Send>)?;
            // `jsonb` wire format: a 1-byte version header, then UTF-8 text.
            out.extend_from_slice(&[1]);
            out.extend_from_slice(&bytes);
            Ok(IsNull::No)
        }
        (value, &Type::JSON) => {
            let bytes = toasty_sql::json::to_vec(value)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Sync + Send>)?;
            out.extend_from_slice(&bytes);
            Ok(IsNull::No)
        }
        // List → bind as a PostgreSQL array via the streaming `array_to_sql`
        // primitive: the closure runs per element and writes directly into
        // `out`, so there's no intermediate `Vec<Option<T>>`. The element PG
        // type (carried by the prepared statement, see `db::Type::List` →
        // `to_postgres_type`) drives per-item conversion via this same fn.
        (stmt::Value::List(items), _) => {
            let Kind::Array(elem) = ty.kind() else {
                return Err(format!("Value::List bound to non-array PG type {ty:?}").into());
            };
            let len = i32::try_from(items.len())
                .map_err(|_| format!("array length {} exceeds i32::MAX", items.len()))?;
            array_to_sql(
                [ArrayDimension {
                    len,
                    lower_bound: 1,
                }],
                elem.oid(),
                items.iter(),
                |v, buf| match value_to_sql(v, elem, buf)? {
                    IsNull::No => Ok(postgres_protocol::IsNull::No),
                    IsNull::Yes => Ok(postgres_protocol::IsNull::Yes),
                },
                out,
            )?;
            Ok(IsNull::No)
        }
        (value, _) => Err(format!(
            "cannot bind {:?} as the PostgreSQL type `{ty}`",
            value.infer_ty()
        )
        .into()),
    }
}

#[cfg(feature = "net")]
fn decode_cidr(raw: &[u8], column: &Column) -> Result<IpCidr> {
    let value = inet_from_sql(raw).map_err(|e| read_error(column, e))?;
    IpCidr::new(value.addr(), value.netmask()).map_err(|e| read_error(column, e))
}

#[cfg(feature = "net")]
fn decode_inet(raw: &[u8], column: &Column) -> Result<IpInet> {
    let value = inet_from_sql(raw).map_err(|e| read_error(column, e))?;
    IpInet::new(value.addr(), value.netmask()).map_err(|e| read_error(column, e))
}

#[cfg(feature = "net")]
fn decode_macaddr(raw: &[u8], column: &Column) -> Result<MacAddr6> {
    macaddr_from_sql(raw)
        .map(MacAddr6::from)
        .map_err(|e| read_error(column, e))
}

#[cfg(feature = "net")]
fn decode_macaddr8(raw: &[u8], column: &Column) -> Result<MacAddr8> {
    let bytes: [u8; 8] = raw
        .try_into()
        .map_err(|_| read_error(column, "invalid MACADDR8 wire length"))?;
    Ok(MacAddr8::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(value: stmt::Value, ty: &Type) -> std::result::Result<(), String> {
        value_to_sql(&value, ty, &mut BytesMut::new())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn i64_binds_to_int4_when_it_fits() {
        assert!(bind(stmt::Value::I64(-7), &Type::INT4).is_ok());
        assert!(bind(stmt::Value::I64(i32::MAX as i64), &Type::INT4).is_ok());
    }

    #[test]
    fn i64_out_of_range_for_int4_is_an_error() {
        let err = bind(stmt::Value::I64(i32::MAX as i64 + 1), &Type::INT4).unwrap_err();
        assert!(err.contains("2147483648"), "{err}");
        assert!(err.contains("int4"), "{err}");
        assert!(bind(stmt::Value::I64(i64::MIN), &Type::INT4).is_err());
    }

    #[test]
    fn u64_out_of_range_for_int8_is_an_error() {
        assert!(bind(stmt::Value::U64(i64::MAX as u64), &Type::INT8).is_ok());
        assert!(bind(stmt::Value::U64(i64::MAX as u64 + 1), &Type::INT8).is_err());
    }

    #[test]
    fn out_of_range_list_element_is_an_error() {
        let list = stmt::Value::List(vec![stmt::Value::I64(1), stmt::Value::I64(i64::MAX)]);
        assert!(bind(list, &Type::INT4_ARRAY).is_err());
    }

    #[test]
    fn f64_beyond_f32_range_is_an_error_for_float4() {
        let err = bind(stmt::Value::F64(1e300), &Type::FLOAT4).unwrap_err();
        assert!(err.contains("float4"), "{err}");
        assert!(bind(stmt::Value::F64(-1e300), &Type::FLOAT4).is_err());
    }

    #[test]
    fn f64_in_range_binds_to_float4() {
        assert!(bind(stmt::Value::F64(1.5), &Type::FLOAT4).is_ok());
        assert!(bind(stmt::Value::F64(f32::MAX as f64), &Type::FLOAT4).is_ok());
        // Non-finite values are not out of range: they keep their meaning.
        assert!(bind(stmt::Value::F64(f64::INFINITY), &Type::FLOAT4).is_ok());
        assert!(bind(stmt::Value::F64(f64::NAN), &Type::FLOAT4).is_ok());
    }

    #[test]
    fn f64_beyond_f32_range_decodes_as_an_error() {
        let err = f64_to_f32(1e300).unwrap_err();
        assert!(err.is_type_conversion(), "{err}");
        assert_eq!(f64_to_f32(1.5).unwrap(), 1.5);
    }
}
