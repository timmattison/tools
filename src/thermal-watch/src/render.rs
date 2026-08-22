//! Draw the live line for one sample.

use colored::Colorize;

use crate::mhz::Mhz;
use crate::powermetrics::Sample;
use crate::report::HOLD_RATIO;

/// The eighths of a block, from one eighth to a full block.
const BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// A clock above this share of the peak of the chip draws green.
const GOOD_RATIO: f64 = 0.95;

/// Draw a horizontal bar `width` cells wide, filled to `ratio`.
///
/// The bar always occupies `width` cells, so a column of bars stays aligned.
/// A ratio outside `0.0` to `1.0` is clamped rather than refused, because the
/// caller draws a live display and a single strange sample must not stop it.
#[must_use]
pub fn bar(_ratio: f64, _width: usize) -> String {
    String::new()
}

/// Draw the live line for one sample.
#[must_use]
pub fn sample_line(_sample: &Sample, _p_max: Mhz) -> String {
    String::new()
}
