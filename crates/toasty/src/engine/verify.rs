use crate::Result;
use crate::engine::lower::relation_expr::EmbedTarget;
use crate::engine::{Engine, upsert};
use toasty_core::Error;
use toasty_core::driver::Capability;
use toasty_core::{
    schema::{
        Schema,
        app::{self, ModelId},
    },
    stmt::{self, Statement, Visit},
};

struct Verify<'a, 'v> {
    schema: &'a Schema,
    capability: &'a Capability,
    error: &'v mut Option<Error>,
}

struct VerifyExpr<'a, 'v> {
    schema: &'a Schema,
    capability: &'a Capability,
    model: ModelId,
    error: &'v mut Option<Error>,
}

/// What an expression path denotes in the application schema. See
/// `VerifyExpr::resolve_expr_path`.
enum PathTarget<'a> {
    /// A field of a model, embed, or relation target.
    Field(&'a app::Field),

    /// The payload of an embedded enum's selected variant.
    Variant(EmbedTarget<'a>),

    /// A position inside a `#[document]` value, which the schema does not
    /// describe field by field.
    Document,
}

impl Engine {
    pub(crate) fn verify(&self, stmt: &Statement) -> Result<()> {
        let mut error = None;
        Verify {
            schema: &self.schema,
            capability: self.capability,
            error: &mut error,
        }
        .visit(stmt);
        match error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

impl stmt::Visit for Verify<'_, '_> {
    fn visit_stmt_insert(&mut self, i: &stmt::Insert) {
        stmt::visit::visit_stmt_insert(self, i);

        let Some(upsert) = &i.upsert else {
            return;
        };
        let model = self
            .schema
            .app
            .model(i.target.model_id_unwrap())
            .as_root_unwrap();
        let stmt::UpsertTarget::Fields(target) = &upsert.target else {
            self.record(Error::invalid_statement(
                "upsert conflict target must contain model fields before lowering",
            ));
            return;
        };
        let target = target
            .iter()
            .filter_map(|projection| projection.as_slice().first().copied())
            .collect::<Vec<_>>();
        // A partial unique index is not a conflict target: the fields are
        // unique only among the rows its predicate admits.
        let Some(index) = model.indices.iter().find(|index| {
            index.is_unique_over_all_rows()
                && index.fields.len() == target.len()
                && index
                    .fields
                    .iter()
                    .zip(&target)
                    .all(|(field, target)| field.field.index == *target)
        }) else {
            self.record(Error::invalid_statement(
                "upsert conflict target must exactly match a unique constraint",
            ));
            return;
        };

        if index.primary_key && !self.capability.upsert_primary_key {
            self.record(Error::unsupported_feature(format!(
                "{} does not support primary-key upsert",
                self.capability.driver_name
            )));
        } else if !index.primary_key && !self.capability.upsert_unique {
            self.record(Error::unsupported_feature(format!(
                "{} does not support upsert by a secondary unique constraint",
                self.capability.driver_name
            )));
        }

        if upsert.action == stmt::UpsertAction::Ignore && !self.capability.upsert_targeted_ignore {
            self.record(Error::unsupported_feature(format!(
                "{} does not support targeted upsert ignore",
                self.capability.driver_name
            )));
        }

        if upsert.action == stmt::UpsertAction::Update
            && !upsert.update.is_empty()
            && !self.capability.upsert_branch_assignments
        {
            self.record(Error::unsupported_feature(format!(
                "{} does not support upsert on_update assignments",
                self.capability.driver_name
            )));
        }

        if upsert.action == stmt::UpsertAction::Update
            && upsert.shared.is_empty()
            && upsert.update.is_empty()
        {
            self.record(Error::invalid_statement(
                "upsert requires at least one update assignment; use or_ignore() instead",
            ));
        }

        for (projection, assignment) in &upsert.shared {
            let has_default = upsert.defaults.contains(projection);
            if upsert::requires_current_value(assignment) && !has_default {
                self.record(Error::invalid_statement(
                    "shared upsert mutations require a field with #[default]; use on_create and on_update instead",
                ));
            }
        }

        if !self.capability.upsert_branch_assignments && upsert.action == stmt::UpsertAction::Update
        {
            for (projection, _) in &upsert.defaults {
                let used = upsert
                    .shared
                    .get(projection)
                    .is_some_and(upsert::requires_current_value)
                    || (!upsert.shared.contains(projection) && !upsert.create.contains(projection));
                if !used {
                    continue;
                }
                let Some(&field) = projection.as_slice().first() else {
                    continue;
                };
                if model.fields[field].nullable {
                    self.record(Error::unsupported_feature(format!(
                        "{} does not support nullable upsert field defaults",
                        self.capability.driver_name
                    )));
                }
            }

            for (projection, _) in &upsert.create {
                let Some(&field) = projection.as_slice().first() else {
                    continue;
                };
                if model.fields[field].nullable {
                    self.record(Error::unsupported_feature(format!(
                        "{} does not support nullable upsert create assignments",
                        self.capability.driver_name
                    )));
                }
                if upsert.shared.contains(projection) {
                    self.record(Error::unsupported_feature(format!(
                        "{} does not support different create and update assignments for one field",
                        self.capability.driver_name
                    )));
                }
            }
        }

        if !self.capability.sql() && upsert.action == stmt::UpsertAction::Update {
            for secondary in model
                .indices
                .iter()
                .filter(|index| index.unique && !index.primary_key)
            {
                if secondary.fields.iter().any(|field| {
                    upsert
                        .shared
                        .keys()
                        .any(|projection| projection.as_slice().first() == Some(&field.field.index))
                        || upsert.create.keys().any(|projection| {
                            projection.as_slice().first() == Some(&field.field.index)
                        })
                        || upsert.defaults.keys().any(|projection| {
                            projection.as_slice().first() == Some(&field.field.index)
                        })
                        || upsert.update.keys().any(|projection| {
                            projection.as_slice().first() == Some(&field.field.index)
                        })
                        || model.fields[field.field.index].auto.is_some()
                }) {
                    self.record(Error::unsupported_feature(format!(
                        "{} upsert does not support updating a unique secondary-index field",
                        self.capability.driver_name
                    )));
                }
            }
        }
    }

    fn visit_stmt_delete(&mut self, i: &stmt::Delete) {
        stmt::visit::visit_stmt_delete(self, i);

        // Reading the removed records back needs `DELETE ... RETURNING`. A
        // returning that reads no column (an empty record per removed row,
        // which is how the removed records are counted) is served from the
        // affected-row count, so every driver accepts it.
        if let Some(returning) = &i.returning
            && !is_empty_record_returning(returning)
            && !self.capability.returning_from_delete
        {
            self.record(Error::unsupported_feature(format!(
                "{} does not support returning deleted records",
                self.capability.driver_name
            )));
        }

        VerifyExpr {
            schema: self.schema,
            model: i.from.model_id_unwrap(),
            capability: self.capability,
            error: &mut *self.error,
        }
        .verify_filter(&i.filter);
    }

    fn visit_stmt_query(&mut self, i: &stmt::Query) {
        stmt::visit::visit_stmt_query(self, i);

        self.verify_single_query(i);
        self.verify_aggregates(i);
        self.verify_order_by(i);
        self.verify_offset_key_matches_order_by(i);
        self.verify_limit_is_integer_literal(i);
    }

    fn visit_stmt_select(&mut self, i: &stmt::Select) {
        stmt::visit::visit_stmt_select(self, i);

        self.verify_include_modifiers(i);

        VerifyExpr {
            schema: self.schema,
            model: i.source.model_id_unwrap(),
            capability: self.capability,
            error: &mut *self.error,
        }
        .verify_filter(&i.filter);
    }

    fn visit_expr_stmt(&mut self, i: &stmt::ExprStmt) {
        // Mutation sub-statements (delete, update, insert) embedded in
        // expressions must have a returning clause so their result can be
        // used as a value. Query sub-statements produce results implicitly.
        if !i.stmt.is_query() {
            assert!(
                i.stmt.returning().is_some(),
                "mutation sub-statement in expression must have a returning clause; stmt={:#?}",
                i.stmt
            );
        }

        stmt::visit::visit_expr_stmt(self, i);
    }

    fn visit_assignments(&mut self, i: &stmt::Assignments) {
        // Builder combinators that cannot express their operation (a
        // `stmt::patch` into an enum variant) record a reason instead of an
        // entry. The statement fails as a whole, before planning.
        for reason in i.unsupported() {
            self.record(Error::unsupported_feature(reason.clone()));
        }

        stmt::visit::visit_assignments(self, i);
    }

    fn visit_stmt_update(&mut self, i: &stmt::Update) {
        stmt::visit::visit_stmt_update(self, i);

        // A rejected assignment may be the only one requested, so there is
        // nothing further to check.
        if !i.assignments.unsupported().is_empty() {
            return;
        }

        // Is not an empty update
        assert!(!i.assignments.is_empty(), "stmt = {i:#?}");

        let mut verify_expr = VerifyExpr {
            schema: self.schema,
            model: i.target.model_id_unwrap(),
            capability: self.capability,
            error: &mut *self.error,
        };

        // The target query was verified above in its own scope. Its include
        // paths may be rooted at enum variants, not at the updated model.
        verify_expr.visit_assignments(&i.assignments);
        verify_expr.visit_filter(&i.filter);
        verify_expr.visit_condition(&i.condition);
    }
}

impl Verify<'_, '_> {
    fn record(&mut self, err: Error) {
        if self.error.is_none() {
            *self.error = Some(err);
        }
    }

    /// `NULLS FIRST` / `NULLS LAST` and an ordering by a related record's
    /// value are SQL. A related value also has no column to read a cursor
    /// from, so it orders an offset page but not a cursor page.
    fn verify_order_by(&mut self, i: &stmt::Query) {
        let Some(order_by) = i.order_by.as_ref() else {
            return;
        };

        let has_nulls = order_by.exprs.iter().any(|expr| expr.nulls.is_some());
        let has_subquery = order_by
            .exprs
            .iter()
            .any(|expr| matches!(expr.expr, stmt::Expr::Stmt(_)));

        if !self.capability.sql() {
            if has_nulls {
                self.record(Error::unsupported_feature(format!(
                    "{} does not support placing NULL values in an ordering (nulls_first / nulls_last)",
                    self.capability.driver_name
                )));
            }
            if has_subquery {
                self.record(Error::unsupported_feature(format!(
                    "{} does not support ordering by a value of a related record",
                    self.capability.driver_name
                )));
            }
            return;
        }

        if has_subquery && matches!(i.limit, Some(stmt::Limit::Cursor(_))) {
            self.record(Error::unsupported_feature(
                "cursor-based pagination cannot order by a value of a related record; \
                 use limit and offset instead",
            ));
        }
    }

    /// Aggregates (`count`, `sum`, `avg`, `min`, `max`), `group_by` and
    /// `having` are SQL. A grouped or aggregating query yields one row per
    /// group, so everything it reads outside an aggregate must be one of its
    /// grouping keys. Grouping keys and aggregate arguments are primitive
    /// fields of the queried model, or of a record reached through
    /// `belongs_to` relations.
    fn verify_aggregates(&mut self, i: &stmt::Query) {
        let stmt::ExprSet::Select(select) = &i.body else {
            return;
        };
        let order_by = i.order_by.as_ref().map(|o| &o.exprs[..]).unwrap_or(&[]);

        let aggregates = select
            .returning
            .as_project()
            .is_some_and(contains_aggregate)
            || select.having.as_ref().is_some_and(contains_aggregate)
            || order_by.iter().any(|expr| contains_aggregate(&expr.expr));
        let in_filter = contains_aggregate(select.filter.as_expr());

        if !aggregates && !in_filter && !select.is_grouped() {
            return;
        }

        if !self.capability.sql() {
            self.record(Error::unsupported_feature(format!(
                "{} does not support aggregate queries (count, sum, avg, min, max, group_by, having)",
                self.capability.driver_name
            )));
            return;
        }

        if in_filter {
            self.record(Error::invalid_statement(
                "an aggregate cannot be used in a filter, which is evaluated per record; \
                 use having to filter groups",
            ));
            return;
        }

        let Some(source) = select.source.as_model() else {
            return;
        };

        if !select.returning.is_project() {
            self.record(Error::invalid_statement(
                "a grouped query yields one row per group; choose what it reads with select(...)",
            ));
            return;
        }

        if matches!(i.limit, Some(stmt::Limit::Cursor(_))) {
            self.record(Error::unsupported_feature(
                "cursor-based pagination of a grouped or aggregating query is not supported; \
                 use limit and offset instead",
            ));
            return;
        }

        for key in &select.group_by {
            if self.group_key_field(source.id, key).is_some() {
                continue;
            }
            let embedded = self
                .resolve_field_path(source.id, key)
                .is_some_and(|(field, _)| matches!(field.ty, app::FieldTy::Embedded(_)));
            self.record(Error::invalid_statement(if embedded {
                "group_by cannot group by an enum with data variants or a whole embedded \
                 struct; group by a unit-only enum, or by the primitive fields inside the embed"
            } else {
                "group_by takes field paths: a primitive field or a unit-only enum of the \
                 queried model, or of a record reached through belongs_to relations, \
                 directly or inside embedded structs"
            }));
            return;
        }

        let mut check = CheckGrouped {
            verify: self,
            model: source.id,
            keys: &select.group_by,
        };
        if let Some(returning) = select.returning.as_project() {
            check.visit_expr(returning);
        }
        if let Some(having) = &select.having {
            check.visit_expr(having);
        }
        for expr in order_by {
            check.visit_expr(&expr.expr);
        }
    }

    /// The field `expr` reads when it is a path to a primitive field of
    /// `model`, directly or through `belongs_to` relations.
    fn column_path_field(&self, model: ModelId, expr: &stmt::Expr) -> Option<&app::Field> {
        let (field, in_embed) = self.resolve_field_path(model, expr)?;
        if in_embed {
            return None;
        }

        match &field.ty {
            app::FieldTy::Primitive(primitive) if !matches!(primitive.ty, stmt::Type::Model(_)) => {
                Some(field)
            }
            _ => None,
        }
    }

    /// The field a grouping key reads: what [`Self::column_path_field`]
    /// takes, a primitive field inside embedded structs, or a unit-only
    /// enum. Each lowers to a single column.
    fn group_key_field(&self, model: ModelId, expr: &stmt::Expr) -> Option<&app::Field> {
        let (field, _) = self.resolve_field_path(model, expr)?;

        match &field.ty {
            app::FieldTy::Primitive(primitive) if !matches!(primitive.ty, stmt::Type::Model(_)) => {
                Some(field)
            }
            app::FieldTy::Embedded(embedded) => match self.schema.app.model(embedded.target) {
                app::Model::EmbeddedEnum(embed) if !embed.has_data_variants() => Some(field),
                _ => None,
            },
            _ => None,
        }
    }

    /// The field at the end of `expr` when it is a field path from `model`:
    /// `belongs_to` relations, then fields of embedded structs. Also returns
    /// whether the path steps into an embedded struct.
    fn resolve_field_path(&self, model: ModelId, expr: &stmt::Expr) -> Option<(&app::Field, bool)> {
        let (index, steps) = match expr {
            stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, index }) => {
                (*index, &[][..])
            }
            stmt::Expr::Project(project) => match &*project.base {
                stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, index }) => {
                    (*index, project.projection.as_slice())
                }
                _ => return None,
            },
            _ => return None,
        };

        let mut field = self.schema.app.model(model).as_root()?.fields.get(index)?;
        let mut in_embed = false;
        for &step in steps {
            field = match &field.ty {
                app::FieldTy::BelongsTo(belongs_to) if !in_embed => self
                    .schema
                    .app
                    .model(belongs_to.target)
                    .as_root()?
                    .fields
                    .get(step)?,
                app::FieldTy::Embedded(embedded) => {
                    let app::Model::EmbeddedStruct(embed) = self.schema.app.model(embedded.target)
                    else {
                        return None;
                    };
                    in_embed = true;
                    embed.fields.get(step)?
                }
                _ => return None,
            };
        }

        Some((field, in_embed))
    }

    fn verify_offset_key_matches_order_by(&mut self, i: &stmt::Query) {
        let Some(stmt::Limit::Cursor(cursor)) = i.limit.as_ref() else {
            return;
        };

        let Some(after) = cursor.after.as_ref() else {
            return;
        };

        // SQL requires ORDER BY for cursor-based pagination.
        // NoSQL drivers (DynamoDB) use a driver-level cursor (ExclusiveStartKey)
        // and do not require ORDER BY.
        if !self.capability.sql() {
            return;
        }

        let Some(order_by) = i.order_by.as_ref() else {
            self.record(Error::invalid_statement(
                "cursor-based pagination requires an ORDER BY clause",
            ));
            return;
        };

        match after {
            stmt::Expr::Value(stmt::Value::Record(record)) => {
                if record.fields.is_empty() {
                    self.record(Error::invalid_statement(
                        "cursor must contain at least one ORDER BY value",
                    ));
                } else if record.fields.len() > order_by.exprs.len() {
                    self.record(Error::invalid_statement(format!(
                        "cursor contains {} values but the query has {} ORDER BY fields",
                        record.fields.len(),
                        order_by.exprs.len(),
                    )));
                }
            }
            // A scalar cursor specifies the first ORDER BY value. This remains
            // valid when normalization appends hidden tie-breaker fields.
            stmt::Expr::Value(_) => {}
            _ => self.record(Error::invalid_statement(
                "cursor must be a literal value or record",
            )),
        }
    }

    /// Reject include ordering on singular relations and preserve the existing
    /// rule that filters are rejected only on required singular relations.
    fn verify_include_modifiers(&mut self, i: &stmt::Select) {
        for include in i.returning.model_includes() {
            let Some(query) = &include.query else {
                continue;
            };
            let has_filter = match &query.body {
                stmt::ExprSet::Select(select) => select.filter.expr.is_some(),
                _ => false,
            };
            let has_order_by = query.order_by.is_some();
            if !has_filter && !has_order_by {
                continue;
            }
            let mut error = None;
            let verifier = VerifyExpr {
                schema: self.schema,
                capability: self.capability,
                model: i.source.model_id_unwrap(),
                error: &mut error,
            };
            let Some(PathTarget::Field(field)) =
                verifier.resolve_expr_path(&include.path.clone().into_stmt())
            else {
                continue;
            };
            let singular = match &field.ty {
                app::FieldTy::Has(rel) => rel.is_one(),
                app::FieldTy::BelongsTo(_) => true,
                app::FieldTy::Via(via) => via.is_one(),
                _ => continue,
            };
            if has_order_by && singular {
                self.record(Error::invalid_statement(format!(
                    "cannot order the include of singular relation `{}`; \
                     include ordering requires a many-valued relation",
                    field.name,
                )));
                continue;
            }
            let required_one = singular && !field.nullable;
            if has_filter && required_one {
                self.record(Error::invalid_statement(format!(
                    "cannot filter the include of required relation `{}`; \
                     filter the parent query instead",
                    field.name,
                )));
                continue;
            }
        }
    }

    fn verify_single_query(&self, i: &stmt::Query) {
        if !i.single {
            return;
        }

        if let stmt::ExprSet::Values(values) = &i.body {
            assert_eq!(1, values.rows.len(), "stmt={i:#?}");
        }
    }

    /// Assert that every field inside a `LIMIT` clause is an `I64` literal.
    ///
    /// Runtime pagination fields use `Expr::Value`; the fixed limit from
    /// `.first()` uses `Expr::Static`. Downstream consumers rely on this
    /// invariant. Any other form means either a builder regressed or the AST was
    /// hand-constructed with a non-canonical shape.
    fn verify_limit_is_integer_literal(&self, i: &stmt::Query) {
        let Some(limit) = i.limit.as_ref() else {
            return;
        };
        match limit {
            stmt::Limit::Cursor(c) => {
                assert_i64_value(&c.page_size, "Cursor page_size");
            }
            stmt::Limit::Offset(o) => {
                assert_i64_literal(&o.limit, "Offset limit");
                if let Some(off) = o.offset.as_ref() {
                    assert_i64_value(off, "Offset offset");
                }
            }
        }
    }
}

/// Checks that a grouped query reads fields only as grouping keys or inside
/// aggregates.
struct CheckGrouped<'a, 'b, 'v> {
    verify: &'b mut Verify<'a, 'v>,
    model: ModelId,
    keys: &'b [stmt::Expr],
}

impl stmt::Visit for CheckGrouped<'_, '_, '_> {
    fn visit_expr(&mut self, expr: &stmt::Expr) {
        if self.keys.contains(expr) {
            return;
        }

        match expr {
            stmt::Expr::Func(stmt::ExprFunc::Count(stmt::FuncCount { arg: None, .. })) => {}
            stmt::Expr::Func(stmt::ExprFunc::Count(stmt::FuncCount { arg: Some(arg), .. }))
            | stmt::Expr::Func(stmt::ExprFunc::Aggregate(stmt::FuncAggregate { arg, .. })) => {
                if contains_aggregate(arg) {
                    self.verify.record(Error::invalid_statement(
                        "an aggregate cannot contain another aggregate",
                    ));
                } else if self.verify.column_path_field(self.model, arg).is_none() {
                    self.verify.record(Error::invalid_statement(
                        "an aggregate takes a field path: a primitive field of the queried \
                         model, or of a record reached through belongs_to relations",
                    ));
                }
            }
            stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, .. })
            | stmt::Expr::Project(_)
                if is_field_path(expr) =>
            {
                let name = self
                    .verify
                    .column_path_field(self.model, expr)
                    .map(|field| format!("`{}` ", field.name))
                    .unwrap_or_default();
                self.verify.record(Error::invalid_statement(format!(
                    "the field {name}is read by a grouped or aggregating query but is neither \
                     a group_by key nor inside an aggregate"
                )));
            }
            // A subquery reads its own records.
            stmt::Expr::Stmt(_) => {}
            _ => stmt::visit::visit_expr(self, expr),
        }
    }
}

/// Whether `expr` is a field path rooted at the current query's model.
fn is_field_path(expr: &stmt::Expr) -> bool {
    match expr {
        stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, .. }) => true,
        stmt::Expr::Project(project) => is_field_path(&project.base),
        _ => false,
    }
}

/// Whether `expr` contains an aggregate of its own query (aggregates inside
/// a subquery belong to the subquery).
fn contains_aggregate(expr: &stmt::Expr) -> bool {
    struct Find(bool);

    impl stmt::Visit for Find {
        fn visit_expr(&mut self, expr: &stmt::Expr) {
            if self.0 || matches!(expr, stmt::Expr::Stmt(_)) {
                return;
            }
            if expr.is_aggregate() {
                self.0 = true;
                return;
            }
            stmt::visit::visit_expr(self, expr);
        }
    }

    let mut find = Find(false);
    find.visit_expr(expr);
    find.0
}

#[track_caller]
fn assert_i64_literal(expr: &stmt::Expr, what: &str) {
    assert!(
        matches!(
            expr,
            stmt::Expr::Value(stmt::Value::I64(_)) | stmt::Expr::Static(stmt::Value::I64(_))
        ),
        "{what} must be an I64 literal; got {expr:#?}"
    );
}

#[track_caller]
fn assert_i64_value(expr: &stmt::Expr, what: &str) {
    assert!(
        matches!(expr, stmt::Expr::Value(stmt::Value::I64(_))),
        "{what} must be a Value::I64 literal; got {expr:#?}"
    );
}

impl<'a> VerifyExpr<'a, '_> {
    fn verify_filter(&mut self, filter: &stmt::Filter) {
        self.assert_bool_expr(filter.as_expr());
        self.visit_expr(filter.as_expr());
    }

    fn record(&mut self, err: Error) {
        if self.error.is_none() {
            *self.error = Some(err);
        }
    }

    /// Whether `expr` references a whole document-stored field of the current
    /// model: a `#[document]` embed (`Type::Model`) or an embed collection
    /// (`List(Model)`).
    fn is_document_field(&self, expr: &stmt::Expr) -> bool {
        let stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, index }) = expr else {
            return false;
        };
        let Some(root) = self.schema.app.model(self.model).as_root() else {
            return false;
        };
        let Some(field) = root.fields.get(*index) else {
            return false;
        };
        let app::FieldTy::Primitive(primitive) = &field.ty else {
            return false;
        };
        let embed_id = match &primitive.ty {
            stmt::Type::Model(id) => *id,
            stmt::Type::List(elem) => match &**elem {
                stmt::Type::Model(id) => *id,
                _ => return false,
            },
            _ => return false,
        };
        matches!(
            self.schema.app.model(embed_id),
            app::Model::EmbeddedStruct(_)
        )
    }

    /// Resolve an expression path — a field reference in the current scope
    /// under `Project` and `Variant` layers — to what it denotes in the
    /// application schema. `None` when the path does not resolve.
    ///
    /// A projection step continues into a struct embed's fields, a relation
    /// target's fields, or the fields of a selected enum variant, by their
    /// variant-local position (see [`EmbedTarget::field_at`]).
    fn resolve_expr_path(&self, expr: &stmt::Expr) -> Option<PathTarget<'a>> {
        match expr {
            stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, index }) => self
                .schema
                .app
                .model(self.model)
                .as_root()?
                .fields
                .get(*index)
                .map(PathTarget::Field),
            stmt::Expr::Project(project) => {
                let mut target = self.resolve_expr_path(&project.base)?;
                for step in project.projection.as_slice() {
                    target = self.resolve_expr_step(target, *step)?;
                }
                Some(target)
            }
            stmt::Expr::Variant(variant) => {
                let PathTarget::Field(field) = self.resolve_expr_path(&variant.base)? else {
                    return None;
                };
                let app::FieldTy::Embedded(embedded) = &field.ty else {
                    return None;
                };
                EmbedTarget::embed(&self.schema.app, embedded.target)?
                    .select(variant.variant)
                    .map(PathTarget::Variant)
            }
            _ => None,
        }
    }

    /// The target one projection step reaches from `target`.
    fn resolve_expr_step(&self, target: PathTarget<'a>, step: usize) -> Option<PathTarget<'a>> {
        use app::FieldTy;

        let field = match target {
            PathTarget::Variant(target) => target.field_at(step),
            PathTarget::Document => return Some(PathTarget::Document),
            PathTarget::Field(field) => match &field.ty {
                FieldTy::Embedded(embedded) => {
                    EmbedTarget::embed(&self.schema.app, embedded.target)?.field_at(step)
                }
                FieldTy::BelongsTo(_) | FieldTy::Has(_) | FieldTy::Via(_) => {
                    let target = field.relation_target_id().expect("relation has a target");
                    self.schema
                        .app
                        .model(target)
                        .as_root_unwrap()
                        .fields
                        .get(step)
                }
                // A `#[document]` embed stores sub-fields in the document
                // type rather than as `app::Field`s; the path was
                // type-checked by the generated accessors.
                FieldTy::Primitive(app::FieldPrimitive {
                    ty: stmt::Type::Model(_),
                    ..
                }) => return Some(PathTarget::Document),
                FieldTy::Primitive(_) => None,
            },
        };

        field.map(PathTarget::Field)
    }

    /// Whether `expr` is an expression path rooted at a field reference in
    /// the current scope — the shape `resolve_expr_path` validates.
    fn is_scoped_expr_path(&self, expr: &stmt::Expr) -> bool {
        match expr {
            stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, .. }) => true,
            stmt::Expr::Project(project) => self.is_scoped_expr_path(&project.base),
            stmt::Expr::Variant(variant) => self.is_scoped_expr_path(&variant.base),
            _ => false,
        }
    }

    fn assert_bool_expr(&self, expr: &stmt::Expr) {
        use stmt::Expr::*;

        match expr {
            And(_)
            | AllOp(_)
            | AnyOp(_)
            | Between(_)
            | BinaryOp(_)
            | Like(_)
            | InList(_)
            | InSubquery(_)
            | Intersects(_)
            | IsNull(_)
            | IsSuperset(_)
            | IsVariant(_)
            | Not(_)
            | Or(_)
            | StartsWith(_)
            | Value(stmt::Value::Bool(_)) => {}
            expr => panic!("Not a bool? {expr:#?}"),
        }
    }
}

impl stmt::Visit for VerifyExpr<'_, '_> {
    fn visit_expr_and(&mut self, i: &stmt::ExprAnd) {
        stmt::visit::visit_expr_and(self, i);

        for expr in &i.operands {
            self.assert_bool_expr(expr);
        }
    }

    fn visit_expr_not(&mut self, i: &stmt::ExprNot) {
        stmt::visit::visit_expr_not(self, i);
        self.assert_bool_expr(&i.expr);
    }

    fn visit_expr_or(&mut self, i: &stmt::ExprOr) {
        stmt::visit::visit_expr_or(self, i);

        for expr in &i.operands {
            self.assert_bool_expr(expr);
        }
    }

    fn visit_projection(&mut self, i: &stmt::Projection) {
        let root = self.schema.app.model(self.model);
        assert!(
            self.schema.app.resolve(root, i).is_some(),
            "invalid projection: {i:?}"
        );
    }

    fn visit_expr_project(&mut self, i: &stmt::ExprProject) {
        // For a path rooted at a field reference in the current scope,
        // validate the whole path against the schema: each step must name a
        // field of the embed, relation target, or selected enum variant it
        // steps into.
        if self.is_scoped_expr_path(&i.base) {
            assert!(
                self.resolve_expr_path(&stmt::Expr::Project(i.clone()))
                    .is_some(),
                "failed to resolve projection: {i:#?}"
            );
        } else {
            // For other base expressions, visit the base but skip projection
            // validation since the projection is relative to the base
            // expression's type.
            self.visit_expr(&i.base);
        }
    }

    fn visit_expr_variant(&mut self, i: &stmt::ExprVariant) {
        // A selection applies to an enum field reached by a scoped path; it
        // must name one of that enum's variants.
        if self.is_scoped_expr_path(&i.base) {
            assert!(
                self.resolve_expr_path(&stmt::Expr::Variant(i.clone()))
                    .is_some(),
                "failed to resolve variant selection: {i:#?}"
            );
        } else {
            self.visit_expr(&i.base);
        }
    }

    fn visit_expr_binary_op(&mut self, i: &stmt::ExprBinaryOp) {
        stmt::visit::visit_expr_binary_op(self, i);

        // Comparing a `#[document]` field against a whole embed value is not
        // yet supported (document value equality is planned — see the design
        // doc). Reject it here with a clear error instead of letting it reach
        // the engine's type inference, which cannot merge a document column
        // with a record value.
        if self.is_document_field(&i.lhs) || self.is_document_field(&i.rhs) {
            self.record(Error::unsupported_feature(
                "comparing a #[document] field to a whole value is not yet supported; \
                 filter on individual fields inside the document instead",
            ));
        }
    }

    fn visit_expr_stmt(&mut self, i: &stmt::ExprStmt) {
        // A query that refers to the row of the statement it is used in
        // (`Query::value_for`) is evaluated by the database only where it
        // orders that statement.
        if let Statement::Query(query) = &*i.stmt
            && refers_to_enclosing_row(query)
        {
            self.record(Error::unsupported_feature(
                "a value of a related record can only be used in order_by",
            ));
            return;
        }

        stmt::visit::visit_expr_stmt(self, i);
    }

    fn visit_expr_in_subquery(&mut self, i: &stmt::ExprInSubquery) {
        // stmt::visit::visit_expr_in_subquery(self, i);

        // Visit **only** the subquery expression
        self.visit(&*i.expr);

        // The subquery is verified independently, sharing the error slot so
        // failures inside it surface to the caller.
        Verify {
            schema: self.schema,
            capability: self.capability,
            error: &mut *self.error,
        }
        .visit(&*i.query);
    }

    fn visit_expr_like(&mut self, i: &stmt::ExprLike) {
        // `.ilike()` is a pass-through to the database's own case-insensitive
        // LIKE operator. Only PostgreSQL has one (`ILIKE`), so reject a
        // case-insensitive match on any other backend rather than silently
        // emitting plain `LIKE`, whose case behavior differs across engines.
        if i.case_insensitive && !self.capability.native_ilike {
            self.record(Error::unsupported_feature(format!(
                "{} does not provide a native ILIKE operator; use like instead",
                self.capability.driver_name
            )));
        }
        stmt::visit::visit_expr_like(self, i);
    }

    fn visit_expr_is_superset(&mut self, i: &stmt::ExprIsSuperset) {
        if !self.capability.native_array_set_predicates && !rhs_is_concrete_list(&i.rhs) {
            self.record(Error::unsupported_feature(format!(
                "{} requires a literal list on the right-hand side of is_superset",
                self.capability.driver_name
            )));
        }
        stmt::visit::visit_expr_is_superset(self, i);
    }

    fn visit_expr_intersects(&mut self, i: &stmt::ExprIntersects) {
        if !self.capability.native_array_set_predicates && !rhs_is_concrete_list(&i.rhs) {
            self.record(Error::unsupported_feature(format!(
                "{} requires a literal list on the right-hand side of intersects",
                self.capability.driver_name
            )));
        }
        stmt::visit::visit_expr_intersects(self, i);
    }
}

/// True when the expression is — or will fold to — a `Value::List` of
/// concrete values. Verify runs before the simplifier, so the user's
/// `vec![…]` still appears as an `Expr::List` of `Expr::Value` items;
/// `fold::expr_list` collapses that shape to `Value::List` during
/// lowering, which is what the driver eventually sees.
fn rhs_is_concrete_list(expr: &stmt::Expr) -> bool {
    match expr {
        stmt::Expr::Value(stmt::Value::List(_)) => true,
        stmt::Expr::List(list) => list
            .items
            .iter()
            .all(|item| matches!(item, stmt::Expr::Value(_))),
        _ => false,
    }
}

/// Whether `returning` projects an empty record: one value per affected row
/// that reads no column.
fn is_empty_record_returning(returning: &stmt::Returning) -> bool {
    matches!(returning, stmt::Returning::Project(stmt::Expr::Record(record)) if record.fields.is_empty())
}

/// Whether `query` reads a field or column of an enclosing statement's row.
fn refers_to_enclosing_row(query: &stmt::Query) -> bool {
    let mut found = false;
    stmt::visit::for_each_expr(query, |expr| {
        if let stmt::Expr::Reference(
            stmt::ExprReference::Field { nesting, .. } | stmt::ExprReference::Model { nesting },
        ) = expr
            && *nesting > 0
        {
            found = true;
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_util::test_schema;
    use toasty_core::driver::Capability;
    use toasty_core::stmt::{Expr, ExprIsSuperset, ExprList, Value};

    fn verify_with(capability: &'static Capability, stmt: Statement) -> Result<()> {
        let schema = test_schema();
        let mut error = None;
        Verify {
            schema: &schema,
            capability,
            error: &mut error,
        }
        .visit(&stmt);
        match error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn verify_expr_with(capability: &'static Capability, expr: &Expr) -> Option<Error> {
        let schema = test_schema();
        let mut error = None;
        // ModelId is only used by projection-checking visitor methods, which
        // these expression-only tests don't trigger.
        VerifyExpr {
            schema: &schema,
            capability,
            model: toasty_core::schema::app::ModelId(0),
            error: &mut error,
        }
        .visit_expr(expr);
        error
    }

    fn is_superset(rhs: Expr) -> Expr {
        Expr::IsSuperset(ExprIsSuperset {
            lhs: Box::new(Expr::arg(0)),
            rhs: Box::new(rhs),
        })
    }

    #[test]
    fn update_with_unsupported_assignment_is_rejected() {
        let mut assignments = stmt::Assignments::new();
        assignments.reject_unsupported("patch into variant");
        let update = stmt::Update {
            target: stmt::UpdateTarget::Model(ModelId(0)),
            assignments,
            filter: stmt::Filter::new(stmt::Expr::from(true)),
            condition: stmt::Condition::default(),
            returning: None,
        };

        let err = verify_with(&Capability::SQLITE, Statement::Update(update))
            .expect_err("expected unsupported_feature error");
        assert!(err.is_unsupported_feature());
    }

    #[test]
    #[should_panic(expected = "Offset offset must be a Value::I64 literal")]
    fn offset_with_non_i64_limit_panics() {
        let mut query = stmt::Query::unit();
        query.limit = Some(stmt::Limit::Offset(stmt::LimitOffset {
            limit: stmt::Value::I64(10).into(),
            offset: Some(stmt::Value::U64(5).into()),
        }));
        verify_with(&Capability::SQLITE, Statement::Query(query)).unwrap();
    }

    #[test]
    fn is_superset_literal_rhs_accepted_on_ddb() {
        let expr = is_superset(Expr::Value(Value::List(vec![Value::I64(1)])));
        assert!(verify_expr_with(&Capability::DYNAMODB, &expr).is_none());
    }

    #[test]
    fn is_superset_pre_fold_expr_list_accepted_on_ddb() {
        // Pre-simplifier shape produced by `is_superset(vec![…])`: an
        // `Expr::List` of `Expr::Value` items. The fold pass will collapse
        // this to `Value::List` during lowering.
        let expr = is_superset(Expr::List(ExprList {
            items: vec![Expr::Value(Value::I64(1)), Expr::Value(Value::I64(2))],
        }));
        assert!(verify_expr_with(&Capability::DYNAMODB, &expr).is_none());
    }

    #[test]
    fn is_superset_non_literal_rhs_rejected_on_ddb() {
        let expr = is_superset(Expr::arg(1));
        let err = verify_expr_with(&Capability::DYNAMODB, &expr)
            .expect("expected unsupported_feature error");
        assert!(err.is_unsupported_feature());
    }

    #[test]
    fn is_superset_non_literal_rhs_accepted_on_sqlite() {
        let expr = is_superset(Expr::arg(1));
        assert!(verify_expr_with(&Capability::SQLITE, &expr).is_none());
    }

    #[test]
    fn ilike_accepted_on_postgresql() {
        let expr = Expr::ilike(Expr::arg(0), Expr::arg(1));
        assert!(verify_expr_with(&Capability::POSTGRESQL, &expr).is_none());
    }

    #[test]
    fn ilike_rejected_on_sqlite() {
        let expr = Expr::ilike(Expr::arg(0), Expr::arg(1));
        let err = verify_expr_with(&Capability::SQLITE, &expr)
            .expect("expected unsupported_feature error");
        assert!(err.is_unsupported_feature());
        assert!(err.to_string().contains(Capability::SQLITE.driver_name));
    }

    #[test]
    fn ilike_rejected_on_mysql() {
        let expr = Expr::ilike(Expr::arg(0), Expr::arg(1));
        let err = verify_expr_with(&Capability::MYSQL, &expr)
            .expect("expected unsupported_feature error");
        assert!(err.is_unsupported_feature());
    }

    #[test]
    fn ilike_rejected_on_dynamodb() {
        let expr = Expr::ilike(Expr::arg(0), Expr::arg(1));
        let err = verify_expr_with(&Capability::DYNAMODB, &expr)
            .expect("expected unsupported_feature error");
        assert!(err.is_unsupported_feature());
    }

    #[test]
    fn case_sensitive_like_accepted_on_sqlite() {
        let expr = Expr::like(Expr::arg(0), Expr::arg(1));
        assert!(verify_expr_with(&Capability::SQLITE, &expr).is_none());
    }
}
