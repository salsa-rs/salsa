use salsa::Database as _;

#[salsa::input]
struct Input {
    number: usize,
}

#[salsa::tracked]
impl Input {
    #[salsa::tracked(return_ref)]
    fn test(self, db: &dyn salsa::Database) -> Vec<String> {
        (0..self.number(db))
            .map(|i| format!("test {}", i))
            .collect()
    }
}

#[salsa::db]
#[derive(Default)]
struct Database {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for Database {}

#[test]
fn invoke() {
    Database::default().attach(|db| {
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
