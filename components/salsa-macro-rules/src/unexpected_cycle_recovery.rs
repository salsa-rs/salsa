// Macro that generates the body of the cycle recovery function
// for the case where no cycle recovery is possible. Must be a macro
// because the signature types must match the particular tracked function.
#[macro_export]
macro_rules! unexpected_cycle_recovery {
    ($db:ident, $value:ident, $count:ident) => {{
        std::mem::drop($db);
        panic!("cannot recover from cycle")
    }};
}

#[macro_export]
macro_rules! unexpected_cycle_initial {
    ($db:ident) => {{
        std::mem::drop($db);
        panic!("no cycle initial value")
    }};
}
