use crate::engine::simplify::fold_literal_insert;
use toasty_core::{
    schema::app::ModelId,
    stmt::{self, Expr, Insert, InsertTarget, Query, Returning, Type, Value, Values},
};

fn insert(target: InsertTarget, rows: Vec<Expr>, returning: Option<Returning>) -> stmt::Statement {
    Insert {
        target,
        source: Query::values(Values::new(rows)),
        upsert: None,
        returning,
    }
    .into()
}

fn rows(stmt: &stmt::Statement) -> &[Expr] {
    let stmt::Statement::Insert(insert) = stmt else {
        panic!("expected an insert");
    };
    let stmt::ExprSet::Values(values) = &insert.source.body else {
        panic!("expected VALUES");
    };
    &values.rows
}

#[test]
fn literal_record_with_default_is_left_as_is() {
    let row = Expr::record([Expr::Default, Expr::Value(Value::from("a"))]);
    let mut stmt = insert(InsertTarget::Model(ModelId(0)), vec![row.clone()], None);

    assert!(fold_literal_insert(&mut stmt));
    assert_eq!(rows(&stmt), [row]);
}

#[test]
fn literal_record_collapses_to_a_value() {
    let row = Expr::record([Expr::Value(Value::from("a")), Expr::Value(Value::I64(1))]);
    let mut stmt = insert(InsertTarget::Model(ModelId(0)), vec![row], None);

    assert!(fold_literal_insert(&mut stmt));
    assert_eq!(
        rows(&stmt),
        [Expr::Value(Value::record_from_vec(vec![
            Value::from("a"),
            Value::I64(1)
        ]))]
    );
}

#[test]
fn casts_around_literals_are_evaluated() {
    let id = uuid::Uuid::from_u128(7);
    let cast = Expr::cast(Expr::Value(Value::Uuid(id)), Type::Bytes);
    let row = Expr::record([cast.clone()]);
    let mut stmt = insert(
        InsertTarget::Model(ModelId(0)),
        vec![row],
        Some(Returning::Expr(cast)),
    );

    let bytes = Value::Bytes(id.as_bytes().to_vec());
    assert!(fold_literal_insert(&mut stmt));
    assert_eq!(
        rows(&stmt),
        [Expr::Value(Value::record_from_vec(vec![bytes.clone()]))]
    );
    let stmt::Statement::Insert(insert) = &stmt else {
        unreachable!();
    };
    assert_eq!(insert.returning, Some(Returning::Expr(Expr::Value(bytes))));
}

#[test]
fn a_cast_that_does_not_fold_needs_the_full_pass() {
    // A document column's lowering cast names its source type and folds only
    // in the simplifier, which knows the schema.
    let cast = Expr::cast_from(
        Expr::Value(Value::record_from_vec(vec![Value::I64(1)])),
        Type::Model(ModelId(0)),
        Type::Object,
    );
    let mut stmt = insert(
        InsertTarget::Model(ModelId(0)),
        vec![Expr::record([cast])],
        None,
    );

    assert!(!fold_literal_insert(&mut stmt));
}

#[test]
fn computed_rows_need_the_full_pass() {
    let row = Expr::record([Expr::Value(Value::from("a")), Expr::arg(0)]);
    let mut stmt = insert(InsertTarget::Model(ModelId(0)), vec![row], None);

    assert!(!fold_literal_insert(&mut stmt));
}

#[test]
fn included_relations_need_the_full_pass() {
    let row = Expr::record([Expr::Value(Value::from("a"))]);
    let mut stmt = insert(
        InsertTarget::Model(ModelId(0)),
        vec![row],
        Some(Returning::Model {
            include: vec![stmt::Include::new(stmt::Path::model(ModelId(0)))],
        }),
    );

    assert!(!fold_literal_insert(&mut stmt));
}

#[test]
fn scoped_inserts_and_other_statements_need_the_full_pass() {
    let row = Expr::record([Expr::Value(Value::from("a"))]);
    let scope = Query::values(Values::new(vec![]));
    let mut stmt = insert(
        InsertTarget::Scope(Box::new(scope)),
        vec![row.clone()],
        None,
    );
    assert!(!fold_literal_insert(&mut stmt));

    let mut stmt = stmt::Statement::Query(Query::values(Values::new(vec![row])));
    assert!(!fold_literal_insert(&mut stmt));
}
