use super::{Name, Statement, alter_type::type_enum_name};

use toasty_core::schema::db::TypeEnum;

/// An `ALTER TYPE ... RENAME TO ...` statement.
#[derive(Debug, Clone)]
pub struct RenameType {
    /// The current type name, qualified by its schema when it has one.
    pub type_name: Name,
    /// The new type name, unqualified: the type stays in its schema.
    pub new_name: String,
}

impl Statement {
    /// Creates an `ALTER TYPE <name> RENAME TO <new_name>` statement.
    ///
    /// # Panics
    ///
    /// Panics if `ty` has no type name.
    pub fn rename_type(ty: &TypeEnum, new_name: &str) -> Self {
        RenameType {
            type_name: type_enum_name(ty),
            new_name: new_name.to_string(),
        }
        .into()
    }
}

impl From<RenameType> for Statement {
    fn from(value: RenameType) -> Self {
        Self::RenameType(value)
    }
}
