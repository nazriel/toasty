use std::fmt;

/// A possibly schema-qualified SQL name (e.g. `"users"` or `"public"."users"`).
///
/// Stores each segment as a separate string. Single-segment names are the
/// common case.
///
/// # Example
///
/// ```
/// use toasty_sql::stmt::Name;
///
/// let name = Name::from("users");
/// assert_eq!(name.to_string(), "users");
/// ```
#[derive(Debug, Clone)]
pub struct Name(pub Vec<String>);

impl Name {
    /// A name qualified by `schema` when it is set (`"schema"."name"`), or the
    /// single-segment `name` when it is not.
    ///
    /// # Example
    ///
    /// ```
    /// use toasty_sql::stmt::Name;
    ///
    /// assert_eq!(Name::qualified(None, "users").0, ["users"]);
    /// assert_eq!(Name::qualified(Some("audit"), "users").0, ["audit", "users"]);
    /// ```
    pub fn qualified(schema: Option<&str>, name: &str) -> Self {
        Self(schema.into_iter().chain([name]).map(String::from).collect())
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(vec![value.into()])
    }
}

impl From<&String> for Name {
    fn from(value: &String) -> Self {
        Self::from(&value[..])
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = "";
        for ident in &self.0 {
            write!(f, "{s}{ident}")?;
            s = ", ";
        }

        Ok(())
    }
}
