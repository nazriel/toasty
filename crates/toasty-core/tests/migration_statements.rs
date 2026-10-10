use toasty_core::schema::db::Migration;

const BREAKPOINT: &str = "\n-- #[toasty::breakpoint]\n";

#[test]
fn splits_on_breakpoints() {
    let migration =
        Migration::new_sql_with_breakpoints(&["CREATE TABLE a (id INTEGER)", "DROP TABLE b"]);
    assert_eq!(
        migration.statements(),
        ["CREATE TABLE a (id INTEGER)", "DROP TABLE b"]
    );
}

#[test]
fn skips_a_chunk_of_only_comments() {
    let sql = format!(
        "CREATE TABLE a (id INTEGER){BREAKPOINT}-- a note for the reader\n   -- and another\n\n{BREAKPOINT}DROP TABLE b\n"
    );
    let migration = Migration::new_sql(sql);
    assert_eq!(
        migration.statements(),
        ["CREATE TABLE a (id INTEGER)", "DROP TABLE b\n"]
    );
}

#[test]
fn skips_a_blank_chunk() {
    let sql = format!("CREATE TABLE a (id INTEGER){BREAKPOINT}  \n\t\n{BREAKPOINT}DROP TABLE b");
    let migration = Migration::new_sql(sql);
    assert_eq!(
        migration.statements(),
        ["CREATE TABLE a (id INTEGER)", "DROP TABLE b"]
    );
}

#[test]
fn keeps_a_chunk_that_mixes_sql_and_comments() {
    let chunk = "-- add the table\nCREATE TABLE a (id INTEGER); -- done\n";
    let migration = Migration::new_sql(format!("{chunk}{BREAKPOINT}DROP TABLE b"));
    assert_eq!(migration.statements(), [chunk, "DROP TABLE b"]);
}

#[test]
fn a_migration_of_only_comments_has_no_statements() {
    let migration = Migration::new_sql("-- nothing to do\n".to_string());
    assert!(migration.statements().is_empty());
}
