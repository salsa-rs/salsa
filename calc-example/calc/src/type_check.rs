use crate::ir::{
    Anchor, Diagnostic, Diagnostics, Expression, Function, FunctionId, Program, Span,
    StatementData, VariableId,
};
use derive_new::new;
#[cfg(test)]
use expect_test::expect;

// ANCHOR: parse_statements
#[salsa::tracked]
pub fn type_check_program(db: &dyn crate::Db, program: Program) {
    for statement in program.statements(db) {
        match &statement.data {
            StatementData::Function(f) => type_check_function(db, *f, program),
            StatementData::Print(e) => CheckExpression::new(db, program, &program, &[]).check(e),
        }
    }
}

#[salsa::tracked]
pub fn type_check_function(db: &dyn crate::Db, function: Function, program: Program) {
    CheckExpression::new(db, program, &function, function.args(db)).check(function.body(db))
}

#[derive(new)]
struct CheckExpression<'w> {
    db: &'w dyn crate::Db,
    program: Program,
    anchor: &'w dyn Anchor,
    names_in_scope: &'w [VariableId],
}

impl CheckExpression<'_> {
    fn check(&self, expression: &Expression) {
        match &expression.data {
            crate::ir::ExpressionData::Op(left, _, right) => {
                self.check(left);
                self.check(right);
            }
            crate::ir::ExpressionData::Number(_) => {}
            crate::ir::ExpressionData::Variable(v) => {
                if !self.names_in_scope.contains(v) {
                    self.report_error(
                        expression.span,
                        format!("the variable `{}` is not declared", v.text(self.db)),
                    );
                }
            }
            crate::ir::ExpressionData::Call(f, args) => {
                if self.find_function(*f).is_none() {
                    self.report_error(
                        expression.span,
                        format!("the function `{}` is not declared", f.text(self.db)),
                    );
                }
                for arg in args {
                    self.check(arg);
                }
            }
        }
    }

    fn find_function(&self, f: FunctionId) -> Option<Function> {
        self.program.find_function(self.db, f)
    }

    fn report_error(&self, span: Span, message: String) {
        let (start, end) = span.absolute_locations(self.db, self.anchor);
        Diagnostics::push(self.db, Diagnostic::new(start, end, message));
    }
}

/// Create a new database with the given source text and parse the result.
/// Returns the statements and the diagnostics generated.
#[cfg(test)]
fn check_string(
    source_text: &str,
    expected_diagnostics: expect_test::Expect,
    edits: &[(&str, expect_test::Expect, expect_test::Expect)],
) {
    use crate::{db::Database, ir::SourceProgram, parser::parse_source_program};

    // Create the database
    let mut db = Database::default().enable_logging();

    // Create the source program
    let source_program = SourceProgram::new(&mut db, source_text.to_string());

    // Invoke the parser
    let program = parse_source_program(&db, source_program);

    // Read out any diagnostics
    expected_diagnostics.assert_debug_eq(&type_check_program::accumulated::<Diagnostics>(
        &db, program,
    ));

    // Clear logs
    db.take_logs();

    // Apply edits and check diagnostics/logs after each one
    for (new_source_text, expected_diagnostics, expected_logs) in edits {
        source_program
            .set_text(&mut db)
            .to(new_source_text.to_string());
        let program = parse_source_program(&db, source_program);
        expected_diagnostics.assert_debug_eq(&type_check_program::accumulated::<Diagnostics>(
            &db, program,
        ));
        expected_logs.assert_debug_eq(&db.take_logs());
    }
}

#[test]
fn check_print() {
    check_string(
        "print 1 + 2",
        expect![[r#"
            []
        "#]],
        &[],
    );
}

#[test]
fn check_bad_variable_in_program() {
    check_string(
        "print a + b",
        expect![[r#"
            [
                Diagnostic {
                    start: Location(
                        6,
                    ),
                    end: Location(
                        8,
                    ),
                    message: "the variable `a` is not declared",
                },
                Diagnostic {
                    start: Location(
                        10,
                    ),
                    end: Location(
                        11,
                    ),
                    message: "the variable `b` is not declared",
                },
            ]
        "#]],
        &[],
    );
}

#[test]
fn check_bad_function_in_program() {
    check_string(
        "print a(22)",
        expect![[r#"
            [
                Diagnostic {
                    start: Location(
                        6,
                    ),
                    end: Location(
                        11,
                    ),
                    message: "the function `a` is not declared",
                },
            ]
        "#]],
        &[],
    );
}

#[test]
fn check_bad_variable_in_function() {
    check_string(
        "
            fn add_one(a) = a + b
            print add_one(22)
        ",
        expect![[r#"
            [
                Diagnostic {
                    start: Location(
                        33,
                    ),
                    end: Location(
                        47,
                    ),
                    message: "the variable `b` is not declared",
                },
            ]
        "#]],
        &[],
    );
}

#[test]
fn check_bad_function_in_function() {
    check_string(
        "
            fn add_one(a) = add_two(a) + b
            print add_one(22)
        ",
        expect![[r#"
            [
                Diagnostic {
                    start: Location(
                        29,
                    ),
                    end: Location(
                        39,
                    ),
                    message: "the function `add_two` is not declared",
                },
                Diagnostic {
                    start: Location(
                        42,
                    ),
                    end: Location(
                        56,
                    ),
                    message: "the variable `b` is not declared",
                },
            ]
        "#]],
        &[],
    );
}

#[test]
fn fix_bad_variable_in_function() {
    check_string(
        "
            fn double(a) = a * b
            fn quadruple(a) = double(double(a))
            print quadruple(2)
        ",
        expect![[r#"
            [
                Diagnostic {
                    start: Location(
                        32,
                    ),
                    end: Location(
                        46,
                    ),
                    message: "the variable `b` is not declared",
                },
            ]
        "#]],
        &[(
            "
                fn double(a) = a * 2
                fn quadruple(a) = double(double(a))
                print quadruple(2)
            ",
            expect![[r#"
                []
            "#]],
            expect![[r#"
                [
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: parse_source_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_function(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                ]
            "#]],
        )],
    );
}

#[test]
fn fix_bug_in_function() {
    check_string(
        "
            fn double(a) = a
            fn quadruple(a) = double(double(a))
            print quadruple(2)
        ",
        expect![[r#"
            []
        "#]],
        &[(
            "
                fn double(a) = a * 2
                fn quadruple(a) = double(double(a))
                print quadruple(2)
            ",
            expect![[r#"
                []
            "#]],
            // Important: quadruple does not get type-checked again, even though (for example)
            // the positions of its various expressions and statements have changed.
            expect![[r#"
                [
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: parse_source_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_function(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                ]
            "#]],
        )],
    );
}

#[test]
fn add_new_function() {
    check_string(
        "
            fn quadruple(a) = a * 4
            print quadruple(2)
        ",
        expect![[r#"
            []
        "#]],
        &[(
            "
                fn double(a) = a * 2
                fn quadruple(a) = a * 4
                print quadruple(2)
            ",
            expect![[r#"
                []
            "#]],
            // Note: `type_function` only executes once, not twice (huzzah),
            // because we don't need to type-check `quadruple` again.
            expect![[r#"
                [
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: parse_source_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_program(0) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_function(1) } }",
                    "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                ]
            "#]],
        )],
    );
}
