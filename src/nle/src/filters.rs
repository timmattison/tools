//! The filters that ship with the app.

pub mod idle_compression;
pub mod time_dilation;

use crate::pipeline::FilterRegistry;

/// A registry that holds every filter that ships with the app.
#[must_use]
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    idle_compression::register(&mut registry);
    time_dilation::register(&mut registry);
    registry
}
