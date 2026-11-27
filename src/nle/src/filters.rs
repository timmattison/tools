//! The filters that ship with the app.

pub mod cursor;
pub mod time_dilation;
pub mod typing_speed;
pub mod viewport;

use crate::pipeline::FilterRegistry;

/// A registry that holds every filter that ships with the app, under its
/// kind.
#[must_use]
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    cursor::register(&mut registry);
    time_dilation::register(&mut registry);
    typing_speed::register(&mut registry);
    viewport::register(&mut registry);
    registry
}

#[cfg(test)]
mod tests;
