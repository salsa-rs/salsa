salsa::query_group! {
    trait MyDatabase: salsa::Database {
        fn my_query(key: ()) -> () {
            type MyQuery;
            use fn another_module::another_name;
        }
    }
}

mod another_module {
    pub(crate) fn another_name(_: &impl crate::MyDatabase, (): ()) -> () {}
}

fn main() {}
