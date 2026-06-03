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
//! the largest always reaches the full block, so the shape is visible regardless
//! of magnitude. A flat series (every value equal, including all-zero) has no
//! shape and renders as an unbroken row of baseline glyphs.

/// The eight block-drawing glyphs used for a sparkline, lowest bar to highest.
///
/// Index 0 (`▁`) is the baseline a min value or an inactive bucket renders at;
/// index 7 (`█`) is the full block a max value reaches. Each glyph is exactly
/// one `char` and display width 1, so a sparkline's column count equals its glyph
/// count — no `unicode-width` measurement is needed at this layer.
pub(crate) const SPARK_GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Render `values` as a sparkline — one glyph per value, auto-scaled to the
/// series' own `min..max`.
///
/// Each value `v` maps to a glyph index by linear interpolation across the
/// series' observed range, rounded to the nearest of the eight levels:
///
/// ```text
/// index = ((v - min) / (max - min) * 7.0).round()   clamped to 0..=7
/// ```
///
/// This pins the two endpoints exactly: the series minimum lands at index 0
/// (`▁`) and the maximum at index 7 (`█`) whenever `min < max`. Rounding (rather
/// than truncating) keeps the mapping symmetric — a value at the mid-point of the
/// range lands on a mid glyph instead of biasing low.
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
    todo!("implemented in the green commit")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baseline glyph, named for readability in the assertions below.
    const BASELINE: char = '▁';
    /// The full-block glyph, the top of the scale.
    const FULL: char = '█';

    #[test]
    fn strictly_increasing_eight_values_map_to_all_eight_glyphs_in_order() {
        // A series that steps evenly from min to max across eight points must hit
        // each level once, in order: index = round(i/7 * 7) = i for i in 0..=7.
        let out = sparkline(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(out, "▁▂▃▄▅▆▇█");
    }

    #[test]
    fn known_mixed_series_maps_by_the_scaling_rule() {
        // [0, 4, 8]: min = 0, max = 8, range = 8.
        //   0 -> round(0/8 * 7) = round(0.0) = 0 -> ▁
        //   4 -> round(4/8 * 7) = round(3.5) = 4 -> ▅   (round-half-away-from-zero)
        //   8 -> round(8/8 * 7) = round(7.0) = 7 -> █
        let out = sparkline(&[0.0, 4.0, 8.0]);
        assert_eq!(out, "▁▅█");
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
    fn min_maps_to_baseline_and_max_maps_to_full_for_any_ranged_series() {
        // For any series with min < max, the smallest value sits at ▁ and the
        // largest reaches █, irrespective of magnitude or sign.
        let series = [-5.0, 100.0, 12.5, -5.0, 99.9, 100.0];
        let out: Vec<char> = sparkline(&series).chars().collect();
        assert_eq!(out.len(), series.len());
        // Positions of the global min (-5.0) must be baseline.
        assert_eq!(out[0], BASELINE);
        assert_eq!(out[3], BASELINE);
        // Positions of the global max (100.0) must be the full block.
        assert_eq!(out[1], FULL);
        assert_eq!(out[5], FULL);
    }

    #[test]
    fn large_magnitude_series_still_spans_full_scale() {
        // Auto-scaling is magnitude-independent: a series in the billions still
        // pins its own min to ▁ and its own max to █.
        let out: Vec<char> = sparkline(&[1_000_000_000.0, 2_000_000_000.0]).chars().collect();
        assert_eq!(out, vec![BASELINE, FULL]);
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
        // max -> full block.
        assert_eq!(out[0], BASELINE);
        assert_eq!(out[2], FULL);
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
