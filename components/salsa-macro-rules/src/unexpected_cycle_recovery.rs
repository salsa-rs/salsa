// Macro that generates the body of the cycle recovery function
// for the case where no cycle recovery is possible. This has to be
// a macro because it can take a variadic number of arguments.
#[macro_export]
macro_rules! unexpected_cycle_recovery {
    ($db:ident, $value:ident, $count:ident, $($other_inputs:ident),*) => {{
        std::mem::drop($db);
        std::mem::drop(($($other_inputs),*));
        panic!("cannot recover from cycle")
    }};
}

#[macro_export]
macro_rules! unexpected_cycle_initial {
    ($db:ident, $($other_inputs:ident),*) => {{
        std::mem::drop($db);
        std::mem::drop(($($other_inputs),*));
        panic!("no cycle initial value")
    }};
}
