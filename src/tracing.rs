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

macro_rules! debug_span {
    ($($x:tt)*) => {
        crate::tracing::span!(DEBUG, $($x)*)
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
        if ::tracing::enabled!(::tracing::Level::$level) {
            let event = {
                #[cold] #[inline(never)] || { ::tracing::event!(::tracing::Level::$level, $($x)*) }
            };

            event();
        }
    }};
}

macro_rules! span {
    ($level:ident, $($x:tt)*) => {{
        if ::tracing::enabled!(::tracing::Level::$level) {
            let span = {
                #[cold] #[inline(never)] || { ::tracing::span!(::tracing::Level::$level, $($x)*) }
            };

            span()
        } else {
            ::tracing::Span::none()
        }
    }};
}

#[expect(unused_imports)]
pub(crate) use {debug, debug_span, event, info, info_span, span, trace, warn_event as warn};
