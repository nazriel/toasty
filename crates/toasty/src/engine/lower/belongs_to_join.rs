//! Reading a related record's field through `belongs_to` relations with a
//! join.
//!
//! `.select(Post::fields().author().name())` reads a field of the record a
//! `belongs_to` relation points at. On SQL, each such path becomes a column
//! of a table joined to the query's own, so the value is read by the same
//! statement instead of by a second query whose rows are merged in memory.
//! The same applies to grouping keys, `HAVING` and `ORDER BY`, which is what
//! lets a query group by a related record's field.

use indexmap::IndexMap;
use toasty_core::{
    schema::{app, mapping},
    stmt::{self, VisitMut},
};

use crate::engine::lower::LowerStatement;

/// The tables joined to a query's own table (slot 0), one per distinct
/// `belongs_to` path, in slot order.
#[derive(Debug, Default)]
pub(super) struct BelongsToJoins {
    /// Each joined path, as its relation fields from the query's model, and
    /// the table slot of the record it reaches.
    slots: IndexMap<Vec<app::FieldId>, usize>,
    tables: Vec<stmt::TableRef>,
    joins: Vec<stmt::Join>,
}

impl LowerStatement<'_, '_> {
    /// Rewrite every field read through `belongs_to` relations in the
    /// query's projection, grouping keys, `HAVING` and ordering into a column
    /// of a joined table, and return the joins to add to the lowered source.
    ///
    /// Runs before the query is lowered: the rewritten expressions are
    /// already table-level, so lowering leaves them as they are. Filters are
    /// not rewritten; they keep their own (subquery) lowering.
    pub(super) fn join_belongs_to_paths(&self, query: &mut stmt::Query) -> Option<BelongsToJoins> {
        if !self.capability().sql() {
            return None;
        }

        let stmt::ExprSet::Select(select) = &mut query.body else {
            return None;
        };
        let stmt::Source::Model(source) = &select.source else {
            return None;
        };
        if source.via.is_some() {
            return None;
        }

        let mut rewrite = RewritePaths {
            schema: self.schema(),
            model: source.id,
            joins: BelongsToJoins::default(),
            decode: false,
        };

        if let stmt::Returning::Project(expr) = &mut select.returning {
            rewrite.decode = true;
            rewrite.visit_expr_mut(expr);
            rewrite.decode = false;
        }
        for key in &mut select.group_by {
            rewrite.visit_expr_mut(key);
        }
        if let Some(having) = &mut select.having {
            rewrite.visit_expr_mut(having);
        }
        // A cursor is read from the ordering's columns, which must be the
        // query's own.
        if !matches!(query.limit, Some(stmt::Limit::Cursor(_)))
            && let Some(order_by) = &mut query.order_by
        {
            for order_by_expr in &mut order_by.exprs {
                rewrite.visit_expr_mut(&mut order_by_expr.expr);
            }
        }

        if rewrite.joins.tables.is_empty() {
            None
        } else {
            Some(rewrite.joins)
        }
    }
}

impl BelongsToJoins {
    /// Add the joined tables to a lowered query's source.
    pub(super) fn apply(self, query: &mut stmt::Query) {
        let stmt::ExprSet::Select(select) = &mut query.body else {
            unreachable!("joins are collected only for a select");
        };
        let stmt::Source::Table(source) = &mut select.source else {
            unreachable!("a lowered select reads a table");
        };
        debug_assert_eq!(source.tables.len(), 1, "source={source:#?}");
        debug_assert_eq!(source.from.len(), 1, "source={source:#?}");

        source.tables.extend(self.tables);
        source.from[0].joins.extend(self.joins);
    }
}

struct RewritePaths<'a> {
    schema: &'a toasty_core::Schema,
    model: app::ModelId,
    joins: BelongsToJoins,
    /// Whether the expression being visited is a value the query returns as
    /// it is, which may decode a whole embed. Anywhere else (a grouping key,
    /// an ordering, an operand) a path must read a single column.
    decode: bool,
}

impl stmt::VisitMut for RewritePaths<'_> {
    fn visit_expr_mut(&mut self, expr: &mut stmt::Expr) {
        if let Some(column) = self.rewrite_path(expr) {
            *expr = column;
            return;
        }
        // The fields of a returned record are returned as they are; the
        // operands of any other expression are not.
        let decode = self.decode;
        self.decode &= matches!(expr, stmt::Expr::Record(_));
        stmt::visit_mut::visit_expr_mut(self, expr);
        self.decode = decode;
    }

    // A nested statement reads its own model; its paths are its own.
    fn visit_expr_stmt_mut(&mut self, _: &mut stmt::ExprStmt) {}
    fn visit_expr_in_subquery_mut(&mut self, i: &mut stmt::ExprInSubquery) {
        self.visit_expr_mut(&mut i.expr);
    }
    fn visit_expr_exists_mut(&mut self, _: &mut stmt::ExprExists) {}
}

impl RewritePaths<'_> {
    /// The table-level expression for `expr` when it reads a field through
    /// one or more `belongs_to` relations of the query's model.
    ///
    /// The field is a primitive, a primitive inside embedded structs, or a
    /// unit-only enum: each reads one column. Where the value is returned as
    /// it is, it may also be a data-carrying enum or a whole embedded struct,
    /// decoded from the joined table's columns as the model's own embeds are.
    /// An embed holding deferred fields or relations is not rewritten: its
    /// loading needs the related record's own statement.
    fn rewrite_path(&mut self, expr: &stmt::Expr) -> Option<stmt::Expr> {
        let stmt::Expr::Project(project) = expr else {
            return None;
        };
        let stmt::Expr::Reference(stmt::ExprReference::Field { nesting: 0, index }) =
            &*project.base
        else {
            return None;
        };

        let mut chain = vec![app::FieldId {
            model: self.model,
            index: *index,
        }];
        let mut model = self.belongs_to_target(chain[0])?;
        let mut steps = project.projection.as_slice();
        while let [step, rest @ ..] = steps
            && let Some(target) = self.belongs_to_target(app::FieldId {
                model,
                index: *step,
            })
        {
            chain.push(app::FieldId {
                model,
                index: *step,
            });
            model = target;
            steps = rest;
        }

        let field = self.embedded_path_field(model, steps)?;
        let mapping = self
            .schema
            .mapping_for(model)
            .resolve_field_mapping(&steps.into())?;

        let (mut value, decoded) = match mapping {
            mapping::Field::Primitive(primitive) => (primitive.column_expr.clone(), false),
            mapping::Field::Enum(field_enum) if !self.has_data_variants(field) => {
                (field_enum.discriminant.column_expr.clone(), false)
            }
            mapping::Field::Enum(field_enum) if self.decode && self.loads_eagerly(field) => {
                (field_enum.default_returning.clone(), true)
            }
            mapping::Field::Struct(field_struct) if self.decode && self.loads_eagerly(field) => {
                (field_struct.default_returning.clone(), true)
            }
            _ => return None,
        };

        let slot = self.join(&chain);
        stmt::visit_mut::for_each_expr_mut(&mut value, |e| {
            if let stmt::Expr::Reference(stmt::ExprReference::Column(col)) = e {
                col.table = slot;
            }
        });

        // Without a related record a LEFT JOIN reads every column as `NULL`,
        // which a single column returns as it is but a decoded embed would
        // read as an unknown discriminant or a record of `NULL`s. Guard the
        // decoding on the joined record's presence, as a nullable embedded
        // struct guards its record on its presence column.
        if decoded
            && chain
                .iter()
                .any(|field_id| self.schema.app.field(*field_id).nullable())
        {
            let table = self.schema.db.table(self.schema.table_id_for(model));
            let key = stmt::Expr::column(stmt::ExprReference::column(
                slot,
                table.primary_key.columns[0].index,
            ));
            value = stmt::Expr::match_expr(
                stmt::Expr::is_null(key),
                vec![stmt::MatchArm {
                    pattern: stmt::Value::Bool(false),
                    expr: value,
                }],
                stmt::Expr::null(),
            );
        }

        Some(value)
    }

    /// The field `steps` reach inside `model`: a field of the model, then
    /// fields of embedded structs.
    fn embedded_path_field(&self, model: app::ModelId, steps: &[usize]) -> Option<&app::Field> {
        let (&first, rest) = steps.split_first()?;
        let mut field = self.schema.app.model(model).as_root()?.fields.get(first)?;
        for &step in rest {
            let app::FieldTy::Embedded(embedded) = &field.ty else {
                return None;
            };
            let app::Model::EmbeddedStruct(embed) = self.schema.app.model(embedded.target) else {
                return None;
            };
            field = embed.fields.get(step)?;
        }
        Some(field)
    }

    /// Whether `field` is an embedded enum with a variant that carries data.
    fn has_data_variants(&self, field: &app::Field) -> bool {
        let app::FieldTy::Embedded(embedded) = &field.ty else {
            return false;
        };
        match self.schema.app.model(embedded.target) {
            app::Model::EmbeddedEnum(embed) => embed.has_data_variants(),
            _ => false,
        }
    }

    /// Whether an embedded field's value is read whole by its default
    /// decoding: neither it nor any field nested in it is deferred or a
    /// relation.
    fn loads_eagerly(&self, field: &app::Field) -> bool {
        match &field.ty {
            app::FieldTy::Primitive(_) => !field.deferred,
            app::FieldTy::Embedded(embedded) => {
                !field.deferred
                    && self
                        .schema
                        .app
                        .model(embedded.target)
                        .fields()
                        .iter()
                        .all(|field| self.loads_eagerly(field))
            }
            app::FieldTy::BelongsTo(_) | app::FieldTy::Has(_) | app::FieldTy::Via(_) => false,
        }
    }

    /// The model a `belongs_to` field points at, or `None` for any other
    /// field.
    fn belongs_to_target(&self, field: app::FieldId) -> Option<app::ModelId> {
        let model = self.schema.app.model(field.model).as_root()?;
        match &model.fields.get(field.index)?.ty {
            app::FieldTy::BelongsTo(belongs_to) => Some(belongs_to.target),
            _ => None,
        }
    }

    /// The table slot of the record `chain` reaches, joining it (and every
    /// record before it on the chain) on first use. A join is `LEFT` once
    /// any relation on the way is optional, so a missing record reads as
    /// `NULL` instead of removing the row; otherwise it is `INNER`.
    fn join(&mut self, chain: &[app::FieldId]) -> usize {
        if let Some(&slot) = self.joins.slots.get(chain) {
            return slot;
        }

        let (&field_id, parent_chain) = chain.split_last().unwrap();
        let parent_slot = if parent_chain.is_empty() {
            0
        } else {
            self.join(parent_chain)
        };

        let field = self.schema.app.field(field_id);
        let belongs_to = field.ty.as_belongs_to().unwrap();
        let optional = chain
            .iter()
            .any(|field_id| self.schema.app.field(*field_id).nullable());

        // The query's own table is slot 0.
        let slot = self.joins.tables.len() + 1;
        self.joins.tables.push(stmt::TableRef::Table(
            self.schema.table_id_for(belongs_to.target),
        ));

        let on = stmt::Expr::and_from_vec(
            belongs_to
                .foreign_key
                .fields
                .iter()
                .map(|fk| {
                    stmt::Expr::eq(
                        super::via_join::raw_column(self.schema, parent_slot, fk.source),
                        super::via_join::raw_column(self.schema, slot, fk.target),
                    )
                })
                .collect(),
        );
        self.joins.joins.push(stmt::Join {
            table: stmt::SourceTableId(slot),
            constraint: if optional {
                stmt::JoinOp::Left(on)
            } else {
                stmt::JoinOp::Inner(on)
            },
        });

        self.joins.slots.insert(chain.to_vec(), slot);
        slot
    }
}
