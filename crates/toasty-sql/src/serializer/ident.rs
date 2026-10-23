use super::{Dialect, Formatter, Period, ToSql};

use toasty_core::schema::db;

pub(super) struct Ident<S>(pub(super) S);

impl<S: AsRef<str>> ToSql for Ident<S> {
    fn to_sql(self, f: &mut Formatter<'_>) {
        let quote = match f.serializer.dialect {
            Dialect::Mysql | Dialect::MariaDb => '`',
            _ => '"',
        };
        let name = self.0.as_ref();

        f.dst.push(quote);
        // A quote character inside the name is escaped by doubling it.
        if name.contains(quote) {
            f.dst
                .push_str(&name.replace(quote, &format!("{quote}{quote}")));
        } else {
            f.dst.push_str(name);
        }
        f.dst.push(quote);
    }
}

/// A table or type name, qualified by its schema when it has one:
/// `"schema"."name"`, or exactly `"name"` when the schema is `None`. Each
/// part is quoted as an [`Ident`].
pub(super) struct QualifiedIdent<'a>(pub(super) Option<&'a str>, pub(super) &'a str);

impl<'a> QualifiedIdent<'a> {
    pub(super) fn table(table: &'a db::Table) -> Self {
        Self(table.schema.as_deref(), &table.name)
    }

    /// A named enum type. Panics if the type has no name.
    pub(super) fn type_enum(ty: &'a db::TypeEnum) -> Self {
        let name = ty
            .name
            .as_deref()
            .expect("a named enum type requires a type name");
        Self(ty.schema.as_deref(), name)
    }
}

impl ToSql for QualifiedIdent<'_> {
    fn to_sql(self, f: &mut Formatter<'_>) {
        let parts = Period(self.0.into_iter().chain([self.1]).map(Ident));
        fmt!(f, parts);
    }
}
