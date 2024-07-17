#[macro_export]
macro_rules! macro_if {
    (true => $($t:tt)*) => {
        $($t)*
    };

    (false => $($t:tt)*) => {
    };

    (if true { $($t:tt)* } else { $($f:tt)*}) => {
        $($t)*
    };

    (if false { $($t:tt)* } else { $($f:tt)*}) => {
        $($f)*
    };
}
