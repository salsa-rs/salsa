//! Wrappers around `tracing` macros that avoid inlining debug machinery into the hot path,
//! as tracing events are typically only enabled for debugging purposes.

macro_rules! trace {
    ($($x:tt)*) => {
        crate::tracing::event!(TRACE, $($x)*)
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

pub(crate) use {debug, debug_span, event, info, span, trace};
