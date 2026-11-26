//! The filter that sets how the cursor looks.

use crate::pipeline::{FilterRegistry, RegionView};
use crate::types::{Event, FilterKind, FilterOptions};

/// The kind of the filter that sets how the cursor looks.
pub const CURSOR: &str = "cursor";

/// Register the cursor filter under `CURSOR`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_in_place(FilterKind::new(CURSOR), filter);
}

/// The cursor filter.
fn filter(
    events: &[Event],
    _region: RegionView,
    _options: &FilterOptions,
) -> Result<Vec<Event>, String> {
    Ok(events.to_vec())
}

#[cfg(test)]
mod tests;
