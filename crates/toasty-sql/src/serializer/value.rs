use super::{Comma, ToSql};

use crate::stmt;

impl ToSql for &stmt::Value {
    fn to_sql(self, f: &mut super::Formatter<'_>) {
        use stmt::Value::*;

        match self {
            Record(value) => {
                let fields = Comma(value.fields.iter());
                fmt!(f, "(" fields ")");
            }
            List(values) => {
                f.dst.push('(');
                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        f.dst.push_str(", ");
                    }
                    value.to_sql(f);
                }
                f.dst.push(')');
            }
            Null => {
                f.dst.push_str("NULL");
            }
            // Inline as a SQL literal (used in DDL contexts like CHECK constraints).
            // DML values are already extracted as Expr::Arg placeholders before
            // reaching the serializer.
            String(s) => {
                f.dst.push('\'');
                // Escape single quotes by doubling them. MySQL and MariaDB also
                // read a backslash as an escape by default, so it doubles there.
                let escape_backslash = f.serializer.is_mysql();
                let needs_escape = |ch: char| ch == '\'' || (escape_backslash && ch == '\\');
                if s.contains(needs_escape) {
                    for ch in s.chars() {
                        if needs_escape(ch) {
                            f.dst.push(ch);
                        }
                        f.dst.push(ch);
                    }
                } else {
                    f.dst.push_str(s);
                }
                f.dst.push('\'');
            }
            I64(n) => {
                use std::fmt::Write;
                write!(f.dst, "{n}").unwrap();
            }
            Bool(b) => {
                f.dst.push_str(if *b { "TRUE" } else { "FALSE" });
            }
            _ => todo!("inline SQL literal for value: {self:?}"),
        }
    }
}
