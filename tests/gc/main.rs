macro_rules! assert_keys {
    ($db:expr, $($query:expr => ($($key:expr),*),)*) => {
        $(
            let entries = $query.in_db(&$db).entries::<Vec<_>>();
            let mut keys = entries.into_iter().map(|e| e.key).collect::<Vec<_>>();
            keys.sort();
            assert_eq!(keys, vec![$($key),*], "query {:?} had wrong keys", $query);
        )*
    };
}

mod db;
mod derived_tests;
mod discard_values;
mod group;
mod interned;
mod log;
mod shallow_constant_tests;
mod volatile_tests;
