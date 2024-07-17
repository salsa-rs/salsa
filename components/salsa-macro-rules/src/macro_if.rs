#[macro_export]
macro_rules! macro_if {
    (true => $($t:tt)*) => {
        $($t)*
    };

    (false => $($t:tt)*) => {
    };
}
