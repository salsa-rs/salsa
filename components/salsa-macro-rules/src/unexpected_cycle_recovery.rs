// Macro that generates the body of the cycle recovery function
// for the case where no cycle recovery is possible. This has to be
// a macro because it can take a variadic number of arguments.
#[macro_export]
macro_rules! unexpected_cycle_recovery {
    ($db:ident, $cycle:ident, $last_provisional_value:ident, $new_value:ident, $($other_inputs:ident),*) => {{
        let (_db, _cycle, _last_provisional_value) = ($db, $cycle, $last_provisional_value);
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

// Macro that generates the body of the cycle recovery function
// for `cycle_result` where we always return the previous (fallback) value.
// This makes the cycle converge immediately after one iteration.
#[macro_export]
macro_rules! cycle_recovery_return_previous {
    ($db:ident, $cycle:ident, $last_provisional_value:ident, $new_value:ident, $($other_inputs:ident),*) => {{
        let (_db, _cycle) = ($db, $cycle);
        std::mem::drop($new_value);
        std::mem::drop(($($other_inputs,)*));
        ::std::clone::Clone::clone($last_provisional_value)
    }};
}
