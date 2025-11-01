// Macro that generates the body of the cycle recovery function
// for the case where no cycle recovery is possible. This has to be
// a macro because it can take a variadic number of arguments.
#[macro_export]
macro_rules! unexpected_cycle_recovery {
    ($db:ident, $id:ident, $last_provisional_value:ident, $new_value:ident, $count:ident, $($other_inputs:ident),*) => {{
        let (_db, _id, _last_provisional_value, _count) = ($db, $id, $last_provisional_value, $count);
        std::mem::drop(($($other_inputs,)*));
        $new_value
    }};
}

#[macro_export]
macro_rules! unexpected_cycle_initial {
    ($db:ident, $id:ident, $($other_inputs:ident),*) => {{
        std::mem::drop($db);
        std::mem::drop(($($other_inputs,)*));
        panic!("no cycle initial value")
    }};
}
