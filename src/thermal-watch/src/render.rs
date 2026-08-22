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
pub fn bar(ratio: f64, width: usize) -> String {
    // A sample that reports nothing gives a ratio that is not a number, and a
    // live display must keep drawing rather than stop on it.
    let clamped = if ratio.is_finite() {
        ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };

    let cells = eighths(clamped, width);
    let full = cells / 8;
    let remainder = cells % 8;

    let mut drawn = String::with_capacity(width * 4);
    for _ in 0..full {
        drawn.push(BLOCKS[7]);
    }
    if remainder > 0 {
        drawn.push(BLOCKS[remainder - 1]);
    }
    for _ in (full + usize::from(remainder > 0))..width {
        drawn.push(' ');
    }
    drawn
}

/// How many eighths of a cell a ratio fills across `width` cells.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped between zero and the cell count before the cast"
)]
fn eighths(ratio: f64, width: usize) -> usize {
    let total = width.saturating_mul(BLOCKS.len());
    (ratio * total as f64).round().clamp(0.0, total as f64) as usize
}

/// How many cells wide the bar of the live display is.
pub const BAR_WIDTH: usize = 24;

/// Draw the live line for one sample.
///
/// The line leads with the time from the start of the run, so a reader can see
/// where in the run a change happened. The bar and the clock follow, then how
/// busy the P-cluster was, the CPU power, and the thermal pressure level.
#[must_use]
pub fn sample_line(sample: &Sample, p_max: Mhz) -> String {
    let ratio = sample.p_freq.map_or(0.0, |clock| clock.ratio_of(p_max));
    let drawn = bar(ratio, BAR_WIDTH);
    let colored_bar = if ratio >= GOOD_RATIO {
        drawn.green()
    } else if ratio >= HOLD_RATIO {
        drawn.yellow()
    } else {
        drawn.red()
    };

    let clock = sample
        .p_freq
        .map_or_else(|| "    idle".to_owned(), |value| format!("{value}"));
    let busy = sample
        .p_active_pct
        .map_or_else(|| "  -- ".to_owned(), |pct| format!("{pct:5.1}%"));
    let watts = sample.cpu_power_mw.map_or_else(
        || "   -- ".to_owned(),
        |mw| format!("{:5.1}W", f64::from(mw) / 1_000.0),
    );

    let seconds = sample.at.as_secs();
    format!(
        "{:02}:{:02}  {colored_bar} {clock} {}  busy {busy}  cpu {watts}  {:?}",
        seconds / 60,
        seconds % 60,
        format!("({:3.0}% of max)", ratio * 100.0).dimmed(),
        sample.pressure,
    )
}
