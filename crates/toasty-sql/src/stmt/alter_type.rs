use super::{Name, Statement};

use toasty_core::schema::db::{EnumVariant, TypeEnum};

/// An `ALTER TYPE ... ADD VALUE '...'` statement.
///
/// PostgreSQL requires a separate `ALTER TYPE` for each new variant. This
/// statement represents adding a single variant to an existing named enum type.
#[derive(Debug, Clone)]
pub struct AlterType {
    /// The name of the enum type to alter, qualified by its schema when it
    /// has one.
    pub type_name: Name,
    /// The new variant to add.
    pub variant: EnumVariant,
}

impl Statement {
    /// Creates an `ALTER TYPE <name> ADD VALUE '<variant>'` statement.
    ///
    /// # Panics
    ///
    /// Panics if `ty` has no type name.
    pub fn alter_type_add_value(ty: &TypeEnum, variant: &EnumVariant) -> Self {
        AlterType {
            type_name: type_enum_name(ty),
            variant: variant.clone(),
        }
        .into()
    }
}

/// The schema-qualified name of a named enum type.
pub(super) fn type_enum_name(ty: &TypeEnum) -> Name {
    let name = ty.name.as_deref().expect("named enum type");
    Name::qualified(ty.schema.as_deref(), name)
}

impl From<AlterType> for Statement {
    fn from(value: AlterType) -> Self {
        Self::AlterType(value)
    }
}
