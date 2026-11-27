//! The filters that ship with the app.

pub mod backspace;
pub mod cursor;
pub mod time_dilation;

use crate::pipeline::FilterRegistry;

/// A registry that holds every filter that ships with the app, under its
/// kind.
#[must_use]
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    backspace::register(&mut registry);
    cursor::register(&mut registry);
    time_dilation::register(&mut registry);
    registry
}

#[cfg(test)]
mod tests;
