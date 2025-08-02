#![cfg(all(feature = "inventory", feature = "accumulator"))]

use std::collections::HashSet;

mod common;
use common::{LogDatabase, LoggerDatabase};
use expect_test::expect;
use salsa::{Accumulator, Setter};
use test_log::test;

#[salsa::input(debug)]
struct File {
    name: String,
    dependencies: Vec<File>,
    issues: Vec<u32>,
}

#[salsa::accumulator]
#[derive(Debug)]
struct Diagnostic(#[allow(dead_code)] String);

#[salsa::tracked(cycle_fn = cycle_fn, cycle_initial = cycle_initial)]
fn check_file(db: &dyn LogDatabase, file: File) -> Vec<u32> {
    db.push_log(format!(
        "check_file(name = {}, issues = {:?})",
        file.name(db),
        file.issues(db)
    ));

    let mut collected_issues = HashSet::<u32>::from_iter(file.issues(db).iter().copied());

    for dep in file.dependencies(db) {
        let issues = check_file(db, dep);
        collected_issues.extend(issues);
    }

    let mut sorted_issues = collected_issues.iter().copied().collect::<Vec<_>>();
    sorted_issues.sort();

    for issue in &sorted_issues {
        Diagnostic(format!("file {}: issue {}", file.name(db), issue)).accumulate(db);
    }

    sorted_issues
}

fn cycle_initial(_db: &dyn LogDatabase, _file: File) -> Vec<u32> {
    vec![]
}

fn cycle_fn(
    _db: &dyn LogDatabase,
    _value: &[u32],
    _count: u32,
    _file: File,
) -> salsa::CycleRecoveryAction<Vec<u32>> {
    salsa::CycleRecoveryAction::Iterate
}

#[test]
fn accumulate_once() {
    let db = LoggerDatabase::default();

    let file = File::new(&db, "fn".to_string(), vec![], vec![1]);
    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = fn, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file fn: issue 1",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}

#[test]
fn accumulate_with_dep() {
    let db = LoggerDatabase::default();

    let file_a = File::new(&db, "file_a".to_string(), vec![], vec![1]);
    let file_b = File::new(&db, "file_b".to_string(), vec![file_a], vec![2]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}

#[test]
fn accumulate_with_cycle() {
    let mut db = LoggerDatabase::default();

    let file_a = File::new(&db, "file_a".to_string(), vec![], vec![1]);
    let file_b = File::new(&db, "file_b".to_string(), vec![file_a], vec![2]);
    file_a.set_dependencies(&mut db).to(vec![file_b]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
            Diagnostic(
                "file file_a: issue 2",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}

#[test]
fn accumulate_with_cycle_second_revision() {
    let mut db = LoggerDatabase::default();

    let file_a = File::new(&db, "file_a".to_string(), vec![], vec![1]);
    let file_b = File::new(&db, "file_b".to_string(), vec![file_a], vec![2]);
    file_a.set_dependencies(&mut db).to(vec![file_b]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
            Diagnostic(
                "file file_a: issue 2",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));

    file_b.set_issues(&mut db).to(vec![2, 3]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_a);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2, 3])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2, 3])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_a: issue 1",
            ),
            Diagnostic(
                "file file_a: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 3",
            ),
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_b: issue 3",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}

#[test]
fn accumulate_add_cycle() {
    let mut db = LoggerDatabase::default();

    let file_a = File::new(&db, "file_a".to_string(), vec![], vec![1]);
    let file_b = File::new(&db, "file_b".to_string(), vec![file_a], vec![2]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));

    file_a.set_dependencies(&mut db).to(vec![file_b]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_a);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_a: issue 1",
            ),
            Diagnostic(
                "file file_a: issue 2",
            ),
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}

#[test]
fn accumulate_remove_cycle() {
    let mut db = LoggerDatabase::default();

    let file_a = File::new(&db, "file_a".to_string(), vec![], vec![1]);
    let file_b = File::new(&db, "file_b".to_string(), vec![file_a], vec![2]);
    file_a.set_dependencies(&mut db).to(vec![file_b]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
            "check_file(name = file_a, issues = [1])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
            Diagnostic(
                "file file_a: issue 2",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));

    file_a.set_dependencies(&mut db).to(vec![]);

    let diagnostics = check_file::accumulated::<Diagnostic>(&db, file_b);
    db.assert_logs(expect![[r#"
        [
            "check_file(name = file_a, issues = [1])",
            "check_file(name = file_b, issues = [2])",
        ]"#]]);

    expect![[r#"
        [
            Diagnostic(
                "file file_b: issue 1",
            ),
            Diagnostic(
                "file file_b: issue 2",
            ),
            Diagnostic(
                "file file_a: issue 1",
            ),
        ]"#]]
    .assert_eq(&format!("{diagnostics:#?}"));
}
