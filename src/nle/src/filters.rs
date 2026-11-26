//! The filters that ship with the app.

pub mod fast_forward;
pub mod time_dilation;

use crate::pipeline::FilterRegistry;

/// A registry that holds every filter that ships with the app.
#[must_use]
pub fn registry() -> FilterRegistry {
    let mut registry = FilterRegistry::new();
    time_dilation::register(&mut registry);
    fast_forward::register(&mut registry);
    registry
}
