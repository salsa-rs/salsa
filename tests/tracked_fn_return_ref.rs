use salsa::Database;

#[salsa::input]
struct Input {
    number: usize,
}

#[salsa::tracked(return_ref)]
fn test(db: &dyn salsa::Database, input: Input) -> salsa::Result<Vec<String>> {
    Ok((0..input.number(db)?)
        .map(|i| format!("test {}", i))
        .collect())
}

#[test]
fn invoke() -> salsa::Result<()> {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = Input::new(db, 3);
        let x: &Vec<String> = test(db, input)?;
        expect_test::expect![[r#"
            [
                "test 0",
                "test 1",
                "test 2",
            ]
        "#]]
        .assert_debug_eq(x);
        Ok(())
    })
}
