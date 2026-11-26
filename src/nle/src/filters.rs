//! The built-in filters of the editor (issue #154).

use crate::pipeline::FilterRegistry;

/// The kind of the filter that shortens the long pauses between events.
pub const PAUSE_REMOVAL: &str = "pause-removal";

/// A registry that holds each built-in filter under its kind.
#[must_use]
pub fn builtin() -> FilterRegistry {
    FilterRegistry::new()
}

#[cfg(test)]
mod tests;
