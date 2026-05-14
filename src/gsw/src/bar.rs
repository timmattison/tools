//! Render a magnitude bar using 8-level Unicode block characters.

/// Empty cell — Unicode light shade.
const EMPTY: char = '░';

/// Eighth-filled block characters, indexed 0..=8 (0 = empty, 8 = full).
const EIGHTHS: [char; 9] = [EMPTY, '▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

/// Render a bar of `width` cells showing `value` scaled against `max`.
///
/// - When `max == 0`, the bar is all empty cells.
/// - When `value >= max`, the bar is fully filled.
/// - Fractional fill is approximated to the nearest eighth, so a value at
///   90% of max produces ~5 full cells and one ~3/8-filled cell at width 6.
pub fn render_bar(value: u32, max: u32, width: usize) -> String {
    if max == 0 || width == 0 {
        return EMPTY.to_string().repeat(width);
    }
    let clamped = value.min(max);
    let ratio = f64::from(clamped) / f64::from(max);
    // Total eighth-cells to fill across the bar.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let total_eighths = (ratio * (width as f64) * 8.0).round() as usize;
    let full_cells = (total_eighths / 8).min(width);
    let partial = total_eighths % 8;
    let mut result = String::with_capacity(width * 4);
    for _ in 0..full_cells {
        result.push('█');
    }
    if full_cells < width && partial > 0 {
        result.push(EIGHTHS[partial]);
        for _ in (full_cells + 1)..width {
            result.push(EMPTY);
        }
    } else {
        for _ in full_cells..width {
            result.push(EMPTY);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_value_renders_all_empty() {
        assert_eq!(render_bar(0, 10, 6), "░░░░░░");
    }

    #[test]
    fn value_equal_to_max_fills_bar() {
        assert_eq!(render_bar(10, 10, 6), "██████");
    }

    #[test]
    fn value_above_max_clamps_to_full() {
        assert_eq!(render_bar(20, 10, 6), "██████");
    }

    #[test]
    fn half_value_fills_half_bar() {
        // ratio 0.5 → 0.5*6*8 = 24 eighths → 3 full + 0 partial
        assert_eq!(render_bar(5, 10, 6), "███░░░");
    }

    #[test]
    fn small_value_renders_fractional_cell() {
        // ratio 0.1 → 0.1*6*8 = 4.8 ≈ 5 eighths → 0 full + 5/8 partial
        assert_eq!(render_bar(1, 10, 6), "▋░░░░░");
    }

    #[test]
    fn near_full_renders_partial_at_end() {
        // ratio 0.9 → 0.9*6*8 = 43.2 ≈ 43 eighths → 5 full + 3/8 partial
        assert_eq!(render_bar(9, 10, 6), "█████▍");
    }

    #[test]
    fn zero_max_renders_all_empty() {
        // Defensive: no division by zero, no panic.
        assert_eq!(render_bar(0, 0, 6), "░░░░░░");
        assert_eq!(render_bar(5, 0, 6), "░░░░░░");
    }

    #[test]
    fn respects_requested_width() {
        assert_eq!(render_bar(10, 10, 3), "███");
        assert_eq!(render_bar(0, 10, 3), "░░░");
        assert_eq!(render_bar(10, 10, 10), "██████████");
    }
}
