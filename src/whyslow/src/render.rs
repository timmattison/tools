//! The lines of text that show a [`Summary`].

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the next commits call the fit helper from render")
)]

use termbar::str_display_width_as_u16;

use crate::verdict::Summary;

/// The widest line that [`render`] writes, in display columns.
pub const WIDTH: usize = 80;

/// Gives the lines that show `summary`.
#[must_use]
pub fn render(_summary: &Summary) -> Vec<String> {
    Vec::new()
}

/// The mark that ends a label that was cut.
const ELLIPSIS: &str = "…";

/// Gives the display width of `text`, in columns.
fn columns(text: &str) -> usize {
    usize::from(str_display_width_as_u16(text))
}

/// Cuts `label` to `width` display columns, or pads it with spaces to that
/// width. A label that is too wide loses whole characters from its end, and
/// it ends with an ellipsis. A character of two columns that does not fit
/// leaves one space before the ellipsis, so the result always fills the
/// column and always ends with the ellipsis.
fn fit(label: &str, width: usize) -> String {
    if columns(label) <= width {
        return pad_right(label, width);
    }
    let budget = width.saturating_sub(columns(ELLIPSIS));
    let mut cut = String::new();
    let mut used = 0;
    for character in label.chars() {
        let next = used + columns(character.encode_utf8(&mut [0; 4]));
        if next > budget {
            break;
        }
        cut.push(character);
        used = next;
    }
    let gap = budget.saturating_sub(used);
    cut.push_str(&" ".repeat(gap));
    cut.push_str(ELLIPSIS);
    cut
}

/// Adds spaces after `text` until it fills `width` display columns.
fn pad_right(text: &str, width: usize) -> String {
    let missing = width.saturating_sub(columns(text));
    format!("{text}{}", " ".repeat(missing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::{CpuState, Pressure};
    use faulte::vm::SwapUsage;

    /// One gibibyte.
    const GIB: u64 = 1 << 30;
    /// A label that fits gets spaces up to the column, counted in display
    /// columns and not in characters.
    #[test]
    fn a_label_that_fits_is_padded_to_the_column() {
        assert_eq!(fit("abc", 6), "abc   ");
        assert_eq!(fit("abc", 3), "abc");
        let padded = fit("日本", 8);
        assert_eq!(padded, "日本    ");
        assert_eq!(columns(&padded), 8);
    }

    /// A label that is too wide ends with an ellipsis, and it fills the
    /// column exactly.
    #[test]
    fn a_label_that_is_too_wide_ends_with_an_ellipsis() {
        let cut = fit("abcdefghij", 6);
        assert_eq!(cut, "abcde…");
        assert_eq!(columns(&cut), 6);
    }

    /// A cut falls on a character boundary and never goes over the column.
    /// A character of two columns that does not fit leaves one space.
    #[test]
    fn a_cut_keeps_whole_characters_and_never_goes_over_the_column() {
        let japanese = fit("日本語のツール", 8);
        assert_eq!(japanese, "日本語 …");
        assert_eq!(columns(&japanese), 8);
        let party = fit("🎉🎉🎉🎉", 6);
        assert_eq!(party, "🎉🎉 …");
        assert_eq!(columns(&party), 6);
        let cafe = fit("café au lait", 6);
        assert_eq!(cafe, "café …");
        assert_eq!(columns(&cafe), 6);
        for width in 1..12 {
            for label in ["日本語のツール", "🎉 party", "café au lait"] {
                assert!(columns(&fit(label, width)) <= width, "{label} in {width}");
            }
        }
    }

    /// An empty label gives only spaces.
    #[test]
    fn an_empty_label_gives_only_spaces() {
        assert_eq!(fit("", 4), "    ");
    }

    /// Makes a summary with the given states and no group lines. The CPU is
    /// 0% idle on 14 processors, and the pressure is warning when the memory
    /// is short, else normal.
    fn summary(cpu_state: CpuState, memory_short: bool) -> Summary {
        let calm = cpu_state == CpuState::Calm;
        Summary {
            cpu_state,
            memory_short,
            idle_percent: if calm { 85 } else { 0 },
            system_percent: 44,
            load: 152.0,
            processors: 14,
            compression_likely: false,
            cpu_groups: Vec::new(),
            unexplained_percent: None,
            pressure: if memory_short {
                Pressure::Warning
            } else {
                Pressure::Normal
            },
            compressor_bytes: 26 * GIB,
            swap: SwapUsage {
                total_bytes: 5 * GIB,
                used_bytes: 4 * GIB,
            },
            memory_groups: Vec::new(),
        }
    }

    /// A full CPU and short memory give one headline that names both.
    #[test]
    fn the_headline_names_a_full_cpu_and_short_memory() {
        let lines = render(&summary(CpuState::Full, true));
        assert_eq!(
            lines.first().map(String::as_str),
            Some("Likely cause: the CPU is full, and the memory is short.")
        );
    }

    /// A busy CPU uses the word busy.
    #[test]
    fn the_headline_names_a_busy_cpu() {
        let lines = render(&summary(CpuState::Busy, false));
        assert_eq!(
            lines.first().map(String::as_str),
            Some("Likely cause: the CPU is busy.")
        );
        let both = render(&summary(CpuState::Busy, true));
        assert_eq!(
            both.first().map(String::as_str),
            Some("Likely cause: the CPU is busy, and the memory is short.")
        );
    }

    /// Short memory alone gives its own headline and no CPU line.
    #[test]
    fn the_headline_names_short_memory_alone() {
        let lines = render(&summary(CpuState::Calm, true));
        assert_eq!(
            lines.first().map(String::as_str),
            Some("Likely cause: the memory is short.")
        );
        assert!(lines.iter().all(|line| !line.starts_with("CPU")));
    }

    /// With no short resource, the output is the two lines that point to the
    /// disk and the clock speed, and nothing else.
    #[test]
    fn a_calm_machine_gets_exactly_two_lines() {
        let lines = render(&summary(CpuState::Calm, false));
        assert_eq!(
            lines,
            vec![
                "The CPU and the memory are not short: 85% idle, pressure normal.",
                "For the disk, run diskhog. For the clock speed, run thermal-watch.",
            ]
        );
    }
}
