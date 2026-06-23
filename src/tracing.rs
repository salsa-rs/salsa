//! Wrappers around `tracing` macros that avoid inlining debug machinery into the hot path,
//! as tracing events are typically only enabled for debugging purposes.

macro_rules! trace {
    ($($x:tt)*) => {
        crate::tracing::event!(TRACE, $($x)*)
    };
}

macro_rules! warn_event {
    ($($x:tt)*) => {
        crate::tracing::event!(WARN, $($x)*)
    };
}

macro_rules! info {
    ($($x:tt)*) => {
        crate::tracing::event!(INFO, $($x)*)
    };
}

macro_rules! debug {
    ($($x:tt)*) => {
        crate::tracing::event!(DEBUG, $($x)*)
    };
}

macro_rules! trace_with_db {
    ($db:expr, $($x:tt)*) => {
        crate::tracing::event_with_db!($db, TRACE, $($x)*)
    };
}

macro_rules! debug_with_db {
    ($db:expr, $($x:tt)*) => {
        crate::tracing::event_with_db!($db, DEBUG, $($x)*)
    };
}

macro_rules! debug_span {
    ($($x:tt)*) => {
        crate::tracing::span!(DEBUG, $($x)*)
    };
}

#[allow(unused_macros)]
macro_rules! debug_span_with_db {
    ($db:expr, $($x:tt)*) => {
        crate::tracing::span_with_db!($db, DEBUG, $($x)*)
    };
}

#[expect(unused_macros)]
macro_rules! info_span {
    ($($x:tt)*) => {
        crate::tracing::span!(INFO, $($x)*)
    };
}

macro_rules! event {
    ($level:ident, $($x:tt)*) => {{
        let event = {
            #[cold] #[inline(never)] || { ::tracing::event!(::tracing::Level::$level, $($x)*) }
        };

        if ::tracing::enabled!(::tracing::Level::$level) {
            event();
        }
    }};
}

macro_rules! event_with_db {
    ($db:expr, $level:ident, $($x:tt)*) => {{
        if ::tracing::enabled!(::tracing::Level::$level) {
            let event = {
                #[cold] #[inline(never)] || {
                    crate::attach($db, || {
                        ::tracing::event!(::tracing::Level::$level, $($x)*)
                    })
                }
            };
            event();
        }
    }};
}

macro_rules! span {
    ($level:ident, $($x:tt)*) => {{
        let span = {
            #[cold] #[inline(never)] || { ::tracing::span!(::tracing::Level::$level, $($x)*) }
        };

        if ::tracing::enabled!(::tracing::Level::$level) {
            span()
        } else {
            ::tracing::Span::none()
        }
    }};
}

#[allow(unused_macros)]
macro_rules! span_with_db {
    ($db:expr, $level:ident, $($x:tt)*) => {{
        if ::tracing::enabled!(::tracing::Level::$level) {
            let span = {
                #[cold] #[inline(never)] || {
                    crate::attach($db, || {
                        ::tracing::span!(::tracing::Level::$level, $($x)*)
                    })
                }
            };
            span()
        } else {
            ::tracing::Span::none()
        }
    }};
}

#[expect(unused_imports)]
pub(crate) use {
    debug, debug_span, debug_span_with_db, debug_with_db, event, event_with_db, info, info_span,
    span, span_with_db, trace, trace_with_db, warn_event as warn,
};
