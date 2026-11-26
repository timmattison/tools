//! The filters that ship with the app.

pub mod cursor;

use crate::pipeline::FilterRegistry;

/// A registry that holds each filter that ships with the app, under its
/// kind.
#[must_use]
pub fn builtin() -> FilterRegistry {
    FilterRegistry::new()
}

#[cfg(test)]
mod tests;
