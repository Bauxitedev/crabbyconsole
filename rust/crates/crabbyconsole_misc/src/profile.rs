//! Defines the profile! macro.
//!
//! It measures how long a method takes to run. It supports async too.
//! (Just gotta make sure `await` is inside the brackets, not outside.)
//!
//! Changelog:
//! v2: added closure for writing to disk
//! v1: profile! 28-05-26

use std::{fmt, time::Duration};

/// Profile a function.
///
/// Note - the local variables here don't need unique variable names, as macro_rules is hygienic
/// Also note - we use double brackets {{ }} to (try to) ensure the expression is kept in its own scope.
#[macro_export]
macro_rules! profile {
    ($label:expr, $expr:expr, $on_duration:expr) => {{
        let start = std::time::Instant::now();
        let result = $expr;
        let duration = start.elapsed();
        ($on_duration)(duration);
        result
    }};

    ($label:expr, $expr:expr) => {
        $crate::profile!($label, $expr, |duration| {
            // This should be relatively fast, since the closure is not a Box<dyn Fn> so no dynamic dispatch occurs.
            // The closure probably gets inlined too.
            tracing::debug!(func = %$label, time = %$crate::profile::Ms(duration), "profile!");
        })
    };

    ($expr:expr) => {{
        $crate::profile!(stringify!($expr), $expr)
    }};
}

/// Little wrapper type so we can format duration always as milliseconds
/// and without " around it (which happens if you simply convert it to a string)
pub struct Ms(pub Duration);

impl fmt::Display for Ms {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.0.as_millis_f64();
        write!(f, "{ms:.3}ms")
    }
}
