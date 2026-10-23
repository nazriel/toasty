use super::{ErrorSet, KeyAttr};

#[derive(Debug, Default)]
pub(crate) struct ModelAttr {
    /// Primary key definition
    pub(crate) key: Option<KeyAttr>,

    /// Model-level secondary index definitions
    pub(crate) indices: Vec<KeyAttr>,

    /// Optional database table name to map the model to
    pub(crate) table: Option<syn::LitStr>,

    /// Optional database schema the table lives in, from
    /// `#[table(schema = "...")]`
    pub(crate) table_schema: Option<syn::LitStr>,
}

impl ModelAttr {
    pub(super) fn populate_from_ast(
        &mut self,
        attrs: &Vec<syn::Attribute>,
        names: &[syn::Ident],
    ) -> syn::Result<()> {
        let mut errs = ErrorSet::new();

        for attr in attrs {
            if attr.path().is_ident("key") {
                if self.key.is_some() {
                    errs.push(syn::Error::new_spanned(attr, "duplicate #[key] attribute"));
                } else {
                    match KeyAttr::from_ast(attr, names) {
                        Ok(key_attr) => {
                            if let Some(filter) = &key_attr.filter {
                                errs.push(syn::Error::new_spanned(
                                    filter,
                                    "a primary key cannot be partial; `filter` is only \
                                     accepted by `#[index(...)]` and `#[unique(...)]`",
                                ));
                            }
                            self.key = Some(key_attr);
                        }
                        Err(e) => errs.push(e),
                    }
                }
            } else if attr.path().is_ident("index") {
                match KeyAttr::from_ast(attr, names) {
                    Ok(index_attr) => self.indices.push(index_attr),
                    Err(e) => errs.push(e),
                }
            } else if attr.path().is_ident("unique") {
                // A struct-level `#[unique(...)]` is a composite unique index. It
                // mirrors `#[index(...)]` (simple and partition/local modes, plus
                // `name = "..."`) but enforces uniqueness across the listed fields.
                match KeyAttr::from_ast(attr, names) {
                    Ok(mut index_attr) => {
                        index_attr.unique = true;
                        self.indices.push(index_attr);
                    }
                    Err(e) => errs.push(e),
                }
            } else if attr.path().is_ident("table") {
                if self.table.is_some() || self.table_schema.is_some() {
                    return Err(syn::Error::new_spanned(attr, "duplicate `table` attribute"));
                }

                // `#[table(schema = "...", name = "...")]`
                if let syn::Meta::List(_) = &attr.meta {
                    let (schema, name) = parse_table_list(attr)?;
                    self.table_schema = schema;
                    self.table = name;
                    continue;
                }

                let syn::Meta::NameValue(meta) = &attr.meta else {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected `table = \"table_name\"`",
                    ));
                };

                let syn::Expr::Lit(lit) = &meta.value else {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected `table = \"table_name\"`",
                    ));
                };

                let syn::Lit::Str(lit) = &lit.lit else {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "expected `table = \"table_name\"`",
                    ));
                };

                self.table = Some(lit.clone());
            }
        }

        if let Some(err) = errs.collect() {
            return Err(err);
        }

        Ok(())
    }
}

/// Parses the list form of the table attribute: `#[table(name = "t")]`,
/// `#[table(schema = "s")]` or `#[table(schema = "s", name = "t")]`. The
/// names are taken whole; a dot is part of a name, not a separator.
fn parse_table_list(
    attr: &syn::Attribute,
) -> syn::Result<(Option<syn::LitStr>, Option<syn::LitStr>)> {
    let mut schema: Option<syn::LitStr> = None;
    let mut name: Option<syn::LitStr> = None;

    attr.parse_nested_meta(|meta| {
        let (key, slot) = if meta.path.is_ident("schema") {
            ("schema", &mut schema)
        } else if meta.path.is_ident("name") {
            ("name", &mut name)
        } else {
            return Err(syn::Error::new_spanned(
                &meta.path,
                "unknown `table` argument; expected `schema` or `name`",
            ));
        };

        if slot.is_some() {
            return Err(syn::Error::new_spanned(
                &meta.path,
                format!("`{key}` specified more than once"),
            ));
        }

        let value: syn::LitStr = meta.value()?.parse()?;

        if value.value().is_empty() {
            return Err(syn::Error::new_spanned(
                &value,
                format!("`{key}` must be a non-empty string"),
            ));
        }

        *slot = Some(value);
        Ok(())
    })?;

    if schema.is_none() && name.is_none() {
        return Err(syn::Error::new_spanned(
            attr,
            "expected `table(name = \"...\")` or `table(schema = \"...\", name = \"...\")`",
        ));
    }

    Ok((schema, name))
}
