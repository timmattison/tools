//! Pure rendering of a numeric series into a one-line Unicode block sparkline.
//!
//! sccache history is just a sequence of per-bucket metric values (produced by
//! the [`crate::history`] ring), and the watch view wants to show each metric's
//! *shape* over the window in a single row of glyphs. This module is the bottom
//! of that pipeline: a pure `&[f64] -> String` with no state, no clock, and no
//! terminal — so it is trivially unit-testable and can never drift with the rest
//! of the UI.
//!
//! The series is **auto-scaled to its own min..max** rather than to any fixed
//! axis, because the metrics differ by orders of magnitude (a handful of cache
//! errors versus thousands of compile requests) and the point of the sparkline
//! is the *trend*, not the absolute height. Scaling each series independently
//! means the smallest value in the window always sits at the baseline glyph and
//! the largest always reaches the tallest bar (`▇` — one eighth shy of a full
//! cell, so adjacent rows never visually merge), keeping the shape visible
//! regardless of magnitude. A flat series (every value equal, including
//! all-zero) has no shape and renders as an unbroken row of baseline glyphs.

/// The seven block-drawing glyphs used for a sparkline, lowest bar to highest.
///
/// Index 0 (`▁`) is the baseline a min value or an inactive bucket renders at;
/// index 6 (`▇`) is the tallest bar a max value reaches. The full block `█` is
/// deliberately excluded: it fills its entire character cell, so a maxed bar
/// would touch the bottom of whatever the row above renders and the two would
/// read as one merged shape — capping at `▇` keeps the top eighth of every cell
/// clear. Each glyph is exactly one `char` and display width 1, so a sparkline's
/// column count equals its glyph count — no `unicode-width` measurement is
/// needed at this layer.
pub(crate) const SPARK_GLYPHS: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];

/// Map a whole-number scale `level` in `0.0..=6.0` to its block glyph.
///
/// The level comes out of [`sparkline`]'s interpolation already `round`ed and
/// clamped to the valid range, so this is a direct lookup — implemented by
/// scanning for the matching integer index rather than a lossy `f64 as usize`
/// cast, which clippy's `cast_possible_truncation`/`cast_sign_loss` lints forbid.
/// Any out-of-band level (which the clamp makes unreachable) falls back to the
/// tallest glyph, so the function is total and never panics.
fn glyph_for_level(level: f64) -> char {
    SPARK_GLYPHS
        .iter()
        .enumerate()
        .find(|(index, _)| (*index as f64) == level)
        .map_or(SPARK_GLYPHS[SPARK_GLYPHS.len() - 1], |(_, &glyph)| glyph)
}

/// Render `values` as a sparkline — one glyph per value, auto-scaled to the
/// series' own `min..max`.
///
/// Each value `v` maps to a glyph index by linear interpolation across the
/// series' observed range, rounded to the nearest of the seven levels:
///
/// ```text
/// index = ((v - min) / (max - min) * 6.0).round()   clamped to 0..=6
/// ```
///
/// This pins the two endpoints exactly: the series minimum lands at index 0
/// (`▁`) and the maximum at index 6 (`▇`) whenever `min < max` — never the full
/// block `█`, which would touch the cell top and visually merge into the row
/// above (see [`SPARK_GLYPHS`]). Rounding (rather than truncating) keeps the
/// mapping symmetric — a value at the mid-point of the range lands on a mid
/// glyph instead of biasing low.
///
/// Degenerate inputs are handled defensively so the watch loop can never panic on
/// live data:
///
/// - **Empty slice** → empty string (nothing to draw).
/// - **All values equal** (including all-zero) → every glyph is the baseline
///   `▁`; there is no range to scale against, so a flat row is correct and the
///   `(max - min)` divisor is never exercised.
/// - **Single value** → a single `▁`; a one-point series has no shape.
/// - **Non-finite values** (`NaN`, `±inf`) → that position renders at the
///   baseline `▁`. Callers are expected to pass finite data; this is
///   belt-and-braces so a stray non-finite value degrades gracefully instead of
///   propagating into a panic or a garbage index.
///
/// The result is a `String` of glyph characters only — no leading/trailing
/// whitespace and no newline.
pub(crate) fn sparkline(values: &[f64]) -> String {
    // The top index into SPARK_GLYPHS, as an f64 for the interpolation below. The
    // seven glyphs span indices 0..=6, so a value's fractional position in the
    // range scales across `0.0..=6.0` and rounds to one of those seven levels.
    const TOP_INDEX: f64 = (SPARK_GLYPHS.len() - 1) as f64;

    // Only finite values participate in the scale: a NaN/±inf would poison min/max
    // (and any comparison with it is false), so they are excluded from range
    // detection and later rendered at the baseline.
    let (min, max) = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });

    // No finite range to scale against — either the series is empty/all-non-finite
    // (min stays +inf, max stays -inf, so `range` is -inf) or it is flat
    // (min == max, so `range` is 0). Either way there is no shape, so every position
    // collapses to the baseline glyph. Returning here also guarantees the
    // `(max - min)` divisor below is strictly positive. `range` is finite-minus
    // -finite (or the ±inf no-finite case), never NaN, so `<= 0.0` is well-defined.
    let range = max - min;
    if range <= 0.0 {
        return SPARK_GLYPHS[0].to_string().repeat(values.len());
    }

    values
        .iter()
        .map(|&v| {
            if !v.is_finite() {
                // Defensive: a stray non-finite value can't be scaled, so it
                // degrades to the baseline rather than producing a garbage index.
                return SPARK_GLYPHS[0];
            }
            // Linear interpolation across the observed range, rounded to the
            // nearest of the seven levels. `round` is half-away-from-zero; clamping
            // the *float* to `0.0..=TOP_INDEX` before any integer conversion folds
            // away both endpoints' floating-point overshoot and keeps the value
            // non-negative — so the resulting index is provably in `0..=6` without a
            // lossy `f64 as usize` cast. The level is whole after `round`, so the
            // glyph lookup just scans for the matching index.
            let level = ((v - min) / range * TOP_INDEX)
                .round()
                .clamp(0.0, TOP_INDEX);
            glyph_for_level(level)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline glyph, named for readability in the assertions below.
    const BASELINE: char = '▁';
    /// The tallest glyph on the scale — `▇`, deliberately NOT the full block
    /// `█`: a full-height bar touches the top of its cell and visually fuses
    /// with the bottom of whatever is rendered on the row above, so the scale
    /// caps one eighth short of the cell.
    const TOP: char = '▇';

    #[test]
    fn strictly_increasing_seven_values_map_to_all_seven_glyphs_in_order() {
        // A series that steps evenly from min to max across seven points must hit
        // each level once, in order: index = round(i/6 * 6) = i for i in 0..=6.
        // The scale tops out at ▇ — never the full block █, which would touch
        // the cell top and merge into the row above.
        let out = sparkline(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(out, "▁▂▃▄▅▆▇");
    }

    #[test]
    fn known_mixed_series_maps_by_the_scaling_rule() {
        // [0, 4, 8]: min = 0, max = 8, range = 8.
        //   0 -> round(0/8 * 6) = round(0.0) = 0 -> ▁
        //   4 -> round(4/8 * 6) = round(3.0) = 3 -> ▄
        //   8 -> round(8/8 * 6) = round(6.0) = 6 -> ▇
        let out = sparkline(&[0.0, 4.0, 8.0]);
        assert_eq!(out, "▁▄▇");
    }

    #[test]
    fn max_value_renders_top_glyph_never_the_full_block() {
        // The full block █ fills its entire character cell, so a maxed bar
        // would touch the bottom of the glyph above it and the two would read
        // as one merged shape. The scale therefore tops out at ▇ and █ must
        // never appear in any output.
        let out = sparkline(&[0.0, 1.0]);
        assert_eq!(out, "▁▇");
        assert!(
            !out.contains('█'),
            "the tallest bar must not touch the cell top: {out:?}"
        );
    }

    #[test]
    fn all_equal_series_renders_all_baseline_no_panic() {
        // A flat series has no range to scale against; every glyph is the
        // baseline and the (max - min) divisor is never exercised (no NaN).
        let out = sparkline(&[3.0, 3.0, 3.0, 3.0]);
        assert_eq!(out, "▁▁▁▁");
        assert!(out.chars().all(|c| c == BASELINE));
    }

    #[test]
    fn all_zero_series_renders_all_baseline() {
        // All-zero is the most common flat case (no activity at all) and must not
        // divide by zero — every bucket renders at baseline.
        let out = sparkline(&[0.0, 0.0, 0.0]);
        assert_eq!(out, "▁▁▁");
    }

    #[test]
    fn single_value_is_single_baseline() {
        // One point has no shape, so it renders at the baseline regardless of
        // magnitude.
        assert_eq!(sparkline(&[42.0]), "▁");
        assert_eq!(sparkline(&[0.0]), "▁");
    }

    #[test]
    fn empty_slice_is_empty_string() {
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn min_maps_to_baseline_and_max_maps_to_top_for_any_ranged_series() {
        // For any series with min < max, the smallest value sits at ▁ and the
        // largest reaches ▇ (the capped top), irrespective of magnitude or sign.
        let series = [-5.0, 100.0, 12.5, -5.0, 99.9, 100.0];
        let out: Vec<char> = sparkline(&series).chars().collect();
        assert_eq!(out.len(), series.len());
        // Positions of the global min (-5.0) must be baseline.
        assert_eq!(out[0], BASELINE);
        assert_eq!(out[3], BASELINE);
        // Positions of the global max (100.0) must be the top glyph.
        assert_eq!(out[1], TOP);
        assert_eq!(out[5], TOP);
    }

    #[test]
    fn large_magnitude_series_still_spans_full_scale() {
        // Auto-scaling is magnitude-independent: a series in the billions still
        // pins its own min to ▁ and its own max to ▇.
        let out: Vec<char> = sparkline(&[1_000_000_000.0, 2_000_000_000.0])
            .chars()
            .collect();
        assert_eq!(out, vec![BASELINE, TOP]);
    }

    #[test]
    fn non_finite_values_render_at_baseline_without_panicking() {
        // Callers guarantee finite input; this is belt-and-braces. A NaN or ±inf
        // at any position must render at the baseline and never panic or produce
        // an out-of-range glyph index.
        let out: Vec<char> = sparkline(&[0.0, f64::NAN, 10.0, f64::INFINITY, f64::NEG_INFINITY])
            .chars()
            .collect();
        assert_eq!(out.len(), 5);
        // Finite endpoints still scale: 0.0 is the min -> baseline, 10.0 is the
        // max -> top glyph.
        assert_eq!(out[0], BASELINE);
        assert_eq!(out[2], TOP);
        // The three non-finite positions degrade to baseline.
        assert_eq!(out[1], BASELINE);
        assert_eq!(out[3], BASELINE);
        assert_eq!(out[4], BASELINE);
    }

    #[test]
    fn all_non_finite_series_is_all_baseline() {
        // If every value is non-finite there is no finite range at all; the whole
        // row must collapse to baseline rather than panic.
        let out = sparkline(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY]);
        assert_eq!(out, "▁▁▁");
    }

    #[test]
    fn every_glyph_is_a_known_spark_glyph() {
        // Defensive invariant: whatever the input, the output only ever contains
        // glyphs from SPARK_GLYPHS — no stray characters from a rounding bug.
        let out = sparkline(&[0.0, 1.3, 9.9, 4.0, 7.7, 2.2]);
        assert!(out.chars().all(|c| SPARK_GLYPHS.contains(&c)));
    }
}
