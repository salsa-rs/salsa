use salsa::Database;

#[salsa::input]
struct Input {
    number: usize,
}

trait Trait {
    fn test(self, db: &dyn salsa::Database) -> &Vec<String>;
}

#[salsa::tracked]
impl Trait for Input {
    #[salsa::tracked(return_ref)]
    fn test(self, db: &dyn salsa::Database) -> Vec<String> {
        (0..self.number(db)).map(|i| format!("test {i}")).collect()
    }
}

#[test]
fn invoke() {
    salsa::DatabaseImpl::new().attach(|db| {
        let input = Input::new(db, 3);
        let x: &Vec<String> = input.test(db);
        expect_test::expect![[r#"
            [
                "test 0",
                "test 1",
                "test 2",
            ]
        "#]]
        .assert_debug_eq(x);
    })
}
