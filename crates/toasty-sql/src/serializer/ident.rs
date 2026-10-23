use super::{Dialect, Formatter, ToSql};

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
