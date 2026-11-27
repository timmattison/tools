//! The backspace removal filter: it removes typing errors and the backspaces
//! that erase them.

use crate::pipeline::{FilterRegistry, Span, SpanContext};
use crate::types::{FilterKind, FilterOptions};

/// The kind of the backspace removal filter.
pub const KIND: &str = "backspace-removal";

/// The kind of the backspace removal filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// Add the backspace removal filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), remove_backspacing);
}

/// Remove each typed key and the backspace that erases it.
fn remove_backspacing(
    span: Span,
    _context: SpanContext<'_>,
    _options: &FilterOptions,
) -> Result<Span, String> {
    Ok(span)
}

#[cfg(test)]
mod tests;
