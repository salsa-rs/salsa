use crate::{
    compile,
    ir::{
        Diagnostic, Diagnostics, Expression, Function, Op, Program, SourceProgram, Span,
        StatementData, VariableId,
    },
};
use derive_new::new;
use ordered_float::OrderedFloat;
use salsa::DebugWithDb;

pub fn evaluate_source_program(
    db: &dyn crate::Db,
    source_program: SourceProgram,
) -> Result<String, Vec<Diagnostic>> {
    let compiled_program = compile::compile(db, source_program);
    let diagnostics = compile::compile::accumulated::<Diagnostics>(db, source_program);
    if diagnostics.is_empty() {
        match evaluate_program(db, compiled_program) {
            Ok(s) => Ok(s),
            Err(d) => Err(vec![d]),
        }
    } else {
        Err(diagnostics)
    }
}

pub fn evaluate_program(db: &dyn crate::Db, program: Program) -> Result<String, Diagnostic> {
    Evaluator::new(db, program, &[]).evaluate_program()
}

#[salsa::tracked]
pub(crate) fn evaluate_function(
    db: &dyn crate::Db,
    program: Program,
    callee: Function,
    inputs: Vec<OrderedFloat<f64>>,
) -> Result<OrderedFloat<f64>, Diagnostic> {
    db.push_log(&mut || {
        format!(
            "evaluate_function({:?}, {:?})",
            callee.name(db).debug(db),
            inputs
        )
    });

    let callee_args = callee.args(db);
    assert_eq!(inputs.len(), callee_args.len());

    let variables: Vec<_> = callee_args.iter().copied().zip(inputs).collect();
    let body = callee.body(db);
    Evaluator::new(db, program, &variables).evaluate_expression(body)
}

#[derive(new)]
struct Evaluator<'data> {
    db: &'data dyn crate::Db,
    program: Program,
    variables: &'data [(VariableId, OrderedFloat<f64>)],
}

impl Evaluator<'_> {
    fn evaluate_program(&mut self) -> Result<String, Diagnostic> {
        use std::fmt::Write;
        let mut output = String::new();
        for statement in self.program.statements(self.db) {
            match &statement.data {
                StatementData::Function(_) => (),
                StatementData::Print(e) => {
                    let v = self.evaluate_expression(e)?;
                    write!(output, "{}", v).unwrap(); // FIXME
                }
            }
        }
        Ok(output)
    }

    fn error(&self, span: Span, message: String) -> Diagnostic {
        let start = span.start(self.db);
        let end = span.end(self.db);
        Diagnostic {
            start,
            end,
            message,
        }
    }

    fn evaluate_expression(
        &mut self,
        expression: &Expression,
    ) -> Result<OrderedFloat<f64>, Diagnostic> {
        match &expression.data {
            crate::ir::ExpressionData::Op(left, op, right) => {
                let left = self.evaluate_expression(left)?;
                let right = self.evaluate_expression(right)?;
                match op {
                    Op::Add => Ok(left + right),
                    Op::Subtract => Ok(left - right),
                    Op::Multiply => Ok(left * right),
                    Op::Divide => Ok(left / right),
                }
            }
            crate::ir::ExpressionData::Number(f) => Ok(*f),
            crate::ir::ExpressionData::Variable(v) => {
                if let Some(value) = self
                    .variables
                    .iter()
                    .find(|pair| &pair.0 == v)
                    .map(|pair| pair.1)
                {
                    return Ok(value);
                }

                return Err(self.error(
                    expression.span,
                    format!("couldn't find a value for `{:?}`", v.text(self.db)),
                ));
            }
            crate::ir::ExpressionData::Call(f, args) => {
                let callee = match self.program.find_function(self.db, *f) {
                    Some(c) => c,
                    None => {
                        return Err(self.error(
                            expression.span,
                            format!("couldn't find a function named `{:?}`", f.text(self.db)),
                        ))
                    }
                };

                let callee_args = callee.args(self.db);
                if args.len() != callee_args.len() {
                    return Err(self.error(
                        expression.span,
                        format!(
                            "`{:?}` expects {} arguments, but {} were provided",
                            f.text(self.db),
                            callee_args.len(),
                            args.len(),
                        ),
                    ));
                }

                let values: Vec<_> = args
                    .iter()
                    .map(|arg| self.evaluate_expression(arg))
                    .collect::<Result<_, _>>()?;

                evaluate_function(self.db, self.program, callee, values)
            }
        }
    }
}

/// Create a new database with the given source text and parse the result.
/// Returns the statements and the diagnostics generated.
#[cfg(test)]
fn check_string(
    source_text: &str,
    expected_output: expect_test::Expect,
    edits: &[(&str, expect_test::Expect, expect_test::Expect)],
) {
    use crate::db::Database;

    // Create the database
    let mut db = Database::default().enable_logging();

    // Create the source program
    let source_program = SourceProgram::new(&mut db, source_text.to_string());
    let output = evaluate_source_program(&db, source_program);
    expected_output.assert_debug_eq(&output.debug(&db));

    // Clear logs
    db.take_logs();

    // Apply edits and check diagnostics/logs after each one
    for (new_source_text, expected_output, expected_logs) in edits {
        source_program
            .set_text(&mut db)
            .to(new_source_text.to_string());
        let output = evaluate_source_program(&db, source_program);
        expected_output.assert_debug_eq(&output.debug(&db));
        expected_logs.assert_debug_eq(&db.take_logs());
    }
}

#[test_log::test]
fn execute_example() {
    use expect_test::expect;

    check_string(
        "
            fn double(a) = a
            fn quadruple(a) = double(double(a))
            print quadruple(2)
        ",
        expect![[r#"
            Ok(
                "2",
            )
        "#]],
        &[
            (
                "
                fn double(a) = a * 2
                fn quadruple(a) = double(double(a))
                print quadruple(2)
            ",
                expect![[r#"
                Ok(
                    "8",
                )
            "#]],
                // Everything gets evaluated.
                expect![[r#"
                    [
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: parse_statements(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_program(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_function(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: evaluate_function(1) } }",
                        "evaluate_function(FunctionId { [salsa id]: 0, text: \"double\" }, [OrderedFloat(2.0)])",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: evaluate_function(0) } }",
                        "evaluate_function(FunctionId { [salsa id]: 1, text: \"quadruple\" }, [OrderedFloat(2.0)])",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: evaluate_function(2) } }",
                        "evaluate_function(FunctionId { [salsa id]: 0, text: \"double\" }, [OrderedFloat(4.0)])",
                    ]
                "#]],
            ),
            (
                "
                fn double(a) = a * 2

                fn quadruple(a) = double(double(a))

                print quadruple(2)
            ",
                expect![[r#"
                Ok(
                    "8",
                )
            "#]],
                // Adding whitespace changes all the spans, but we don't have to re-evaluate each function.
                // We do have to search the list of statements, though, so `find_function` re-executes.
                expect![[r#"
                    [
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: parse_statements(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: type_check_program(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(0) } }",
                        "Event: Event { runtime_id: RuntimeId { counter: 0 }, kind: WillExecute { database_key: find_function(1) } }",
                    ]
                "#]],
            ),
        ],
    );
}
