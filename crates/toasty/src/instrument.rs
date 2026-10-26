//! Tracing spans for statement execution.
//!
//! Statements execute on the caller's task, but not always there alone: an
//! operation whose caller stops waiting finishes on a spawned task, and the
//! SQLite driver runs each call on the blocking thread pool. The spans
//! created here are constructed on the caller's task — parenting them to the
//! caller's current span (e.g. a per-request span) — and the operation
//! carries its span wherever it runs, so every event the engine and driver
//! emit lands inside the caller's span tree.

use toasty_core::{
    Schema,
    schema::app::ModelId,
    stmt::{self, ExprSet, InsertTarget, UpdateTarget},
};

/// Creates the `toasty::query` span for one statement execution.
///
/// When the span is disabled by the subscriber's filter, returns the
/// caller's current span instead so request context still reaches events
/// emitted off the caller's task.
pub(crate) fn query_span(schema: &Schema, stmt: &stmt::Statement) -> tracing::Span {
    let span = tracing::debug_span!(
        target: "toasty::query",
        "query",
        stmt.kind = stmt.name(),
        model = tracing::field::Empty,
    );
    if span.is_disabled() {
        return tracing::Span::current();
    }
    if let Some(model) = stmt_model_id(schema, stmt).and_then(|id| schema.app.get_model(id)) {
        let name = model.name().upper_camel_case();
        span.record("model", name.as_str());
    }
    span
}

/// Creates the `toasty::query` span for a user-authored SQL execution.
/// Falls back to the caller's current span like [`query_span`].
pub(crate) fn raw_sql_span() -> tracing::Span {
    let span = tracing::debug_span!(target: "toasty::query", "query", stmt.kind = "raw_sql");
    if span.is_disabled() {
        return tracing::Span::current();
    }
    span
}

/// Resolves the model a statement targets, when it targets one directly.
/// Statements arriving here are model-level; lowered (table-level) shapes
/// return `None`.
fn stmt_model_id(schema: &Schema, stmt: &stmt::Statement) -> Option<ModelId> {
    match stmt {
        stmt::Statement::Query(query) => query_model_id(schema, query),
        stmt::Statement::Insert(insert) => match &insert.target {
            InsertTarget::Model(model_id) => Some(*model_id),
            InsertTarget::Scope(query) => query_model_id(schema, query),
            InsertTarget::Table(_) => None,
        },
        stmt::Statement::Update(update) => match &update.target {
            UpdateTarget::Model(model_id) => Some(*model_id),
            UpdateTarget::Query(query) => query_model_id(schema, query),
            UpdateTarget::Table(_) => None,
        },
        stmt::Statement::Delete(delete) => delete
            .from
            .as_model()
            .map(|source| source_model_id(schema, source)),
    }
}

fn query_model_id(schema: &Schema, query: &stmt::Query) -> Option<ModelId> {
    match &query.body {
        ExprSet::Select(select) => select
            .source
            .as_model()
            .map(|source| source_model_id(schema, source)),
        ExprSet::Update(update) => match &update.target {
            UpdateTarget::Model(model_id) => Some(*model_id),
            UpdateTarget::Query(query) => query_model_id(schema, query),
            UpdateTarget::Table(_) => None,
        },
        _ => None,
    }
}

/// The model a source reads from. A scalar-terminal `via` query carries a
/// placeholder id until lowering resolves it, so a source reached through a
/// `via` field reports that field's resolved target instead.
fn source_model_id(schema: &Schema, source: &stmt::SourceModel) -> ModelId {
    source
        .via
        .as_ref()
        .and_then(|association| schema.app.resolve_via_path(&association.path))
        .map_or(source.id, |via| via.target)
}
