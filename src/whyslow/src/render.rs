//! The lines of text that show a [`Summary`].

use termbar::str_display_width_as_u16;

use crate::verdict::{CpuState, GroupLine, Share, Summary};

/// The widest line that [`render`] writes, in display columns.
pub const WIDTH: usize = 80;

/// Gives the lines that show `summary`, with no blank line. The first line is
/// the headline. A calm machine gets two lines that point to other tools.
#[must_use]
pub fn render(summary: &Summary) -> Vec<String> {
    let cpu = summary.cpu_state != CpuState::Calm;
    if !cpu && !summary.memory_short {
        return vec![
            format!(
                "The CPU and the memory are not short: {}% idle, pressure {}.",
                summary.idle_percent,
                summary.pressure.name()
            ),
            POINTER_LINE.to_owned(),
        ];
    }
    let mut lines = vec![headline(summary)];
    if cpu {
        lines.extend(cpu_block(summary));
    }
    if summary.memory_short {
        lines.extend(memory_block(summary));
        if summary.memory_groups.iter().any(has_lower_bound) {
            lines.push(FOOTER_LINE.to_owned());
        }
    }
    lines
}

/// The line that explains the mark of a lower bound.
const FOOTER_LINE: &str = "≥ counts the RSS of other accounts. Run with sudo for the footprint.";

/// The mark in front of a memory value that is a lower bound.
const LOWER_BOUND_MARK: &str = "≥";

/// The number of bytes in one GiB.
const BYTES_PER_GIB: f64 = 1_073_741_824.0;

/// The width of the label of a group line, in display columns.
const GROUP_LABEL_WIDTH: usize = 26;

/// The width of the label of a worktree line, in display columns.
const WORKTREE_LABEL_WIDTH: usize = 39;

/// The width of the label of the short-lived line, in display columns.
const SHORT_LIVED_LABEL_WIDTH: usize = 41;

/// The width of the process count of a group line.
const COUNT_WIDTH: usize = 5;

/// The width of the noun that follows the process count.
const NOUN_WIDTH: usize = 9;

/// The width of the value at the end of a line. Each value ends in the same
/// column: 2 + 26 + 5 + 1 + 9 + 11 = 4 + 39 + 11 = 2 + 41 + 11 = 54. The
/// widest value, such as `≥100.0 GiB`, is 10 columns, so each value keeps
/// one space before it.
const VALUE_WIDTH: usize = 11;

/// The indent of a group line.
const GROUP_INDENT: &str = "  ";

/// The indent of a worktree line and of the line of the rest.
const WORKTREE_INDENT: &str = "    ";

/// The label of the line for the CPU that no group explains. `ps` does not
/// list `kernel_task`, and the kernel refuses `proc_pid_rusage` for PID 0
/// without root. Thus the CPU that no group explains holds the kernel as
/// well as the processes that start and stop between the two snapshots. On
/// 2026-11-15 `kernel_task` alone used 245%.
const SHORT_LIVED_LABEL: &str = "Kernel and short-lived processes";

/// The note under the header when the system time and the memory both point
/// to compression.
const COMPRESSION_LINE: &str =
    "        High system time and short memory: likely compression and swap.";

/// Gives the lines of the CPU block: the header, the compression note, the
/// groups with their worktrees, and the short-lived line.
fn cpu_block(summary: &Summary) -> Vec<String> {
    let noun = if summary.processors == 1 {
        "processor"
    } else {
        "processors"
    };
    let mut lines = vec![format!(
        "CPU     {}% idle, {}% system, load {:.0} on {} {noun}",
        summary.idle_percent, summary.system_percent, summary.load, summary.processors
    )];
    if summary.compression_likely {
        lines.push(COMPRESSION_LINE.to_owned());
    }
    for group in &summary.cpu_groups {
        lines.push(group_line(group, &percent(group.cpu_percent)));
        lines.extend(worktree_lines(group, |share| percent(share.cpu_percent)));
    }
    if let Some(unexplained) = summary.unexplained_percent {
        lines.push(format!(
            "{GROUP_INDENT}{}{}",
            fit(SHORT_LIVED_LABEL, SHORT_LIVED_LABEL_WIDTH),
            pad_left(&percent(unexplained), VALUE_WIDTH)
        ));
    }
    lines
}

/// Gives the lines of the memory block: the header, and the groups with their
/// worktrees.
fn memory_block(summary: &Summary) -> Vec<String> {
    let mut lines = vec![format!(
        "Memory  pressure {}, {} compressed, swap {} of {} in use",
        summary.pressure.name(),
        gib(summary.compressor_bytes),
        number_of_gib(summary.swap.used_bytes),
        gib(summary.swap.total_bytes),
    )];
    for group in &summary.memory_groups {
        lines.push(group_line(
            group,
            &memory_value(group.memory_bytes, group.at_least),
        ));
        lines.extend(worktree_lines(group, |share| {
            memory_value(share.memory_bytes, share.at_least)
        }));
    }
    lines
}

/// Is true when the line, or one of its worktree lines, is a lower bound.
fn has_lower_bound(group: &GroupLine) -> bool {
    group.at_least
        || group.worktrees.iter().any(|share| share.at_least)
        || group.more.as_ref().is_some_and(|(_, rest)| rest.at_least)
}

/// Writes a memory value, with the mark of a lower bound when it is one.
fn memory_value(bytes: u64, at_least: bool) -> String {
    let mark = if at_least { LOWER_BOUND_MARK } else { "" };
    format!("{mark}{}", gib(bytes))
}

/// Writes a number of bytes as GiB with one decimal, as `24.2 GiB`.
fn gib(bytes: u64) -> String {
    format!("{} GiB", number_of_gib(bytes))
}

/// Writes a number of bytes as GiB with one decimal and no unit.
fn number_of_gib(bytes: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "one decimal of GiB needs far fewer bits than an f64 holds"
    )]
    let value = bytes as f64 / BYTES_PER_GIB;
    format!("{value:.1}")
}

/// Writes a percent of one processor, as `290%`.
fn percent(value: u64) -> String {
    format!("{value}%")
}

/// Adds spaces before `text` until it fills `width` display columns.
fn pad_left(text: &str, width: usize) -> String {
    let missing = width.saturating_sub(columns(text));
    format!("{}{text}", " ".repeat(missing))
}

/// Writes one group line with `value` at its end.
fn group_line(group: &GroupLine, value: &str) -> String {
    let noun = if group.processes == 1 {
        "process"
    } else {
        "processes"
    };
    let accounts = if group.accounts > 1 {
        format!("   {} accounts", group.accounts)
    } else {
        String::new()
    };
    format!(
        "{GROUP_INDENT}{}{} {}{}{accounts}",
        fit(&group.label, GROUP_LABEL_WIDTH),
        pad_left(&group.processes.to_string(), COUNT_WIDTH),
        pad_right(noun, NOUN_WIDTH),
        pad_left(value, VALUE_WIDTH),
    )
}

/// Writes the worktree lines of a group, and the line of the rest. Only the
/// compilers have them. `value` writes the value of one share.
fn worktree_lines(group: &GroupLine, value: impl Fn(&Share) -> String) -> Vec<String> {
    let mut lines: Vec<String> = group
        .worktrees
        .iter()
        .map(|share| worktree_line(&share.label, &value(share)))
        .collect();
    if let Some((count, rest)) = &group.more {
        let noun = if *count == 1 { "worktree" } else { "worktrees" };
        lines.push(worktree_line(&format!("{count} more {noun}"), &value(rest)));
    }
    lines
}

/// Writes one worktree line, or the line of the rest.
fn worktree_line(label: &str, value: &str) -> String {
    format!(
        "{WORKTREE_INDENT}{}{}",
        fit(label, WORKTREE_LABEL_WIDTH),
        pad_left(value, VALUE_WIDTH)
    )
}

/// The second line of a machine with no short resource.
const POINTER_LINE: &str = "For the disk, run diskhog. For the clock speed, run thermal-watch.";

/// Gives the headline of a machine with at least one short resource.
fn headline(summary: &Summary) -> String {
    let cpu = match summary.cpu_state {
        CpuState::Full => Some("full"),
        CpuState::Busy => Some("busy"),
        CpuState::Calm => None,
    };
    let cause = match (cpu, summary.memory_short) {
        (Some(word), true) => format!("the CPU is {word}, and the memory is short"),
        (Some(word), false) => format!("the CPU is {word}"),
        (None, _) => "the memory is short".to_owned(),
    };
    format!("Likely cause: {cause}.")
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
    use crate::fixtures;
    use crate::verdict::{summarize, CpuState, GroupLine, Pressure, Share};
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

    /// Makes a group line with one account and no worktree.
    fn line(label: &str, processes: usize, cpu_percent: u64) -> GroupLine {
        GroupLine {
            label: label.to_owned(),
            processes,
            accounts: 1,
            cpu_percent,
            memory_bytes: 0,
            at_least: false,
            worktrees: Vec::new(),
            more: None,
        }
    }

    /// Makes a share for a worktree line.
    fn share(label: &str, cpu_percent: u64) -> Share {
        Share {
            label: label.to_owned(),
            cpu_percent,
            memory_bytes: 0,
            at_least: false,
        }
    }

    /// The header line of the CPU block states the idle part, the system
    /// part, the load, and the number of processors. One processor is the
    /// singular.
    #[test]
    fn the_cpu_header_states_idle_system_load_and_processors() {
        let lines = render(&summary(CpuState::Full, false));
        assert_eq!(
            lines.get(1).map(String::as_str),
            Some("CPU     0% idle, 44% system, load 152 on 14 processors")
        );
        let mut one = summary(CpuState::Full, false);
        one.processors = 1;
        assert_eq!(
            render(&one).get(1).map(String::as_str),
            Some("CPU     0% idle, 44% system, load 152 on 1 processor")
        );
    }

    /// The compression line shows only when the summary says that
    /// compression is likely.
    #[test]
    fn the_compression_line_shows_only_when_compression_is_likely() {
        let compression = "        High system time and short memory: likely compression and swap.";
        let mut likely = summary(CpuState::Full, true);
        likely.compression_likely = true;
        assert_eq!(
            render(&likely).get(2).map(String::as_str),
            Some(compression)
        );
        let unlikely = render(&summary(CpuState::Full, true));
        assert!(!unlikely.iter().any(|l| l == compression));
    }

    /// A group line has the label, the count with its noun, and the value.
    /// A second account adds its own note at the end.
    #[test]
    fn a_group_line_shows_the_count_the_noun_and_the_value() {
        let mut full = summary(CpuState::Full, false);
        let mut claude = line("Claude Code", 176, 290);
        claude.accounts = 2;
        full.cpu_groups = vec![claude, line("WindowServer", 1, 75)];
        let lines = render(&full);
        assert_eq!(
            lines.get(2).map(String::as_str),
            Some("  Claude Code                 176 processes       290%   2 accounts")
        );
        assert_eq!(
            lines.get(3).map(String::as_str),
            Some("  WindowServer                  1 process          75%")
        );
    }

    /// The worktree lines follow the compiler group at once. The line of the
    /// rest says `1 more worktree` or `3 more worktrees`. The next group
    /// comes after them.
    #[test]
    fn the_worktree_lines_follow_the_compiler_group() {
        let mut full = summary(CpuState::Full, false);
        let mut compilers = line("Compilers and linkers", 68, 322);
        compilers.worktrees = vec![share("w/issue-17", 166), share("w/issue-36", 118)];
        compilers.more = Some((1, share("", 38)));
        full.cpu_groups = vec![compilers, line("Claude Code", 2, 10)];
        let lines = render(&full);
        assert_eq!(
            lines.iter().skip(2).map(String::as_str).collect::<Vec<_>>(),
            vec![
                "  Compilers and linkers        68 processes       322%",
                "    w/issue-17                                    166%",
                "    w/issue-36                                    118%",
                "    1 more worktree                                38%",
                "  Claude Code                   2 processes        10%",
            ]
        );
        let mut many = summary(CpuState::Full, false);
        let mut compilers = line("Compilers and linkers", 9, 9);
        compilers.more = Some((3, share("", 9)));
        many.cpu_groups = vec![compilers];
        assert!(render(&many)
            .iter()
            .any(|l| l.contains("    3 more worktrees ")));
    }

    /// The short-lived line comes last in the CPU block, and only when the
    /// summary holds a number for it.
    #[test]
    fn the_short_lived_line_comes_last_in_the_cpu_block() {
        let mut full = summary(CpuState::Full, false);
        full.cpu_groups = vec![line("Claude Code", 2, 10)];
        assert!(!render(&full).iter().any(|l| l.contains("short-lived")));
        full.unexplained_percent = Some(688);
        let lines = render(&full);
        assert_eq!(
            lines.last().map(String::as_str),
            Some("  Kernel and short-lived processes                688%")
        );
    }

    /// Makes a group line for the memory block, with one account.
    fn memory_line(label: &str, bytes: u64, at_least: bool) -> GroupLine {
        GroupLine {
            memory_bytes: bytes,
            at_least,
            ..line(label, 1, 0)
        }
    }

    /// The header of the memory block states the pressure, the compressor,
    /// and the swap, with the unit of the swap once, after the total.
    #[test]
    fn the_memory_header_states_pressure_compressor_and_swap() {
        let mut short = summary(CpuState::Calm, true);
        short.swap = SwapUsage {
            total_bytes: 5_368_709_120,
            used_bytes: 4_187_593_113,
        };
        let lines = render(&short);
        assert_eq!(
            lines.get(1).map(String::as_str),
            Some("Memory  pressure warning, 26.0 GiB compressed, swap 3.9 of 5.0 GiB in use")
        );
    }

    /// A memory value has one decimal of GiB. A lower bound has `≥` in front,
    /// and the value still ends in the same column as the others.
    #[test]
    fn a_memory_line_shows_gib_and_marks_a_lower_bound() {
        let mut short = summary(CpuState::Calm, true);
        short.memory_groups = vec![
            memory_line("Claude Code", 24 * GIB + GIB / 5, true),
            memory_line("wezterm-gui", GIB * 9 / 10, false),
        ];
        let lines = render(&short);
        assert_eq!(
            lines.iter().skip(2).map(String::as_str).collect::<Vec<_>>(),
            vec![
                "  Claude Code                   1 process    ≥24.2 GiB",
                "  wezterm-gui                   1 process      0.9 GiB",
                "≥ counts the RSS of other accounts. Run with sudo for the footprint.",
            ]
        );
    }

    /// The footer shows when a memory line that is shown has a lower bound,
    /// also a worktree line. A lower bound in a CPU line does not cause it.
    #[test]
    fn the_footer_follows_the_lower_bounds_of_the_memory_lines() {
        let footer = "≥ counts the RSS of other accounts. Run with sudo for the footprint.";
        let mut cpu_only = summary(CpuState::Full, true);
        cpu_only.cpu_groups = vec![memory_line("WindowServer", 1, true)];
        cpu_only.memory_groups = vec![memory_line("g", GIB, false)];
        assert!(!render(&cpu_only).iter().any(|l| l == footer));

        let mut worktree = summary(CpuState::Calm, true);
        let mut compilers = memory_line("Compilers and linkers", 2 * GIB, false);
        compilers.worktrees = vec![Share {
            label: "w/issue-17".to_owned(),
            cpu_percent: 0,
            memory_bytes: GIB,
            at_least: true,
        }];
        worktree.memory_groups = vec![compilers];
        let lines = render(&worktree);
        assert_eq!(lines.last().map(String::as_str), Some(footer));
        assert!(lines.iter().any(|l| l.ends_with("≥1.0 GiB")));
    }

    /// A memory value of 10 columns, such as `≥100.0 GiB`, keeps one space
    /// after the noun. The noun `processes` fills its column, so a value that
    /// fills its own column too joins the two words into one.
    #[test]
    fn a_wide_memory_value_keeps_a_space_after_the_noun() {
        let mut short = summary(CpuState::Calm, true);
        short.memory_groups = vec![GroupLine {
            processes: 176,
            ..memory_line("Claude Code", 100 * GIB, true)
        }];
        let lines = render(&short);
        let group = lines.get(2).map(String::as_str).unwrap_or_default();
        assert!(group.contains("processes ≥100.0 GiB"), "{group}");
    }

    /// The column where each value ends.
    const VALUE_END: usize = 54;

    /// Every value ends at the same display column, with labels that are
    /// ASCII, Japanese, emoji, or accented, and with labels that are too
    /// wide. A padding by `char` count would break this.
    #[test]
    fn every_value_ends_in_the_same_display_column() {
        let labels = [
            "Claude Code",
            "日本語のツール",
            "🎉 party",
            "café",
            "日本語のツールの名前がとても長いので切ります",
        ];
        let mut full = summary(CpuState::Full, true);
        full.unexplained_percent = Some(688);
        full.cpu_groups = labels.iter().map(|l| line(l, 3, 100)).collect();
        let mut compilers = line("Compilers and linkers", 9, 9);
        compilers.worktrees = vec![share("日本語/🎉 issue-17", 5), share("café/issue-36", 4)];
        compilers.more = Some((3, share("", 9)));
        full.cpu_groups.push(compilers);
        full.memory_groups = labels
            .iter()
            .map(|l| memory_line(l, 2 * GIB, l.len() % 2 == 0))
            .collect();
        let mut memory_compilers = memory_line("Compilers and linkers", 2 * GIB, false);
        memory_compilers.worktrees = vec![Share {
            label: "日本語/🎉 issue-17".to_owned(),
            cpu_percent: 0,
            memory_bytes: GIB,
            at_least: true,
        }];
        memory_compilers.more = Some((1, share("", 0)));
        full.memory_groups.push(memory_compilers);
        let lines = render(&full);
        let valued: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("  ") && !l.starts_with("        "))
            .collect();
        assert_eq!(valued.len(), 18);
        for line in valued {
            assert_eq!(columns(line), VALUE_END, "{line}");
        }
    }

    /// The most lines that the sample of 2026-11-15 gives.
    const MOST_LINES: usize = 18;

    /// The sample of 2026-11-15 gives the screen that a person needed: the
    /// CPU is full, the memory is short, and the compilers come first with
    /// their two worktrees. The memory block starts with Claude Code as a
    /// lower bound. Each line fits the width. The screen has 18 lines, because
    /// the memory block lists the worktrees of the compilers too.
    #[test]
    fn the_sample_of_2026_11_15_gives_the_screen() {
        let lines = render(&summarize(&fixtures::sample_2026_11_15()));
        assert_eq!(
            lines.first().map(String::as_str),
            Some("Likely cause: the CPU is full, and the memory is short.")
        );
        let position = |needle: &str| lines.iter().position(|l| l.contains(needle));
        let compilers = position("Compilers and linkers").expect("compilers line");
        assert!(position("Claude Code").expect("claude line") > 0);
        assert!(lines[compilers + 1].contains("prediction-markets-worktrees/issue-17"));
        assert!(lines[compilers + 2].contains("prediction-markets-worktrees/issue-36"));
        let memory = lines
            .iter()
            .position(|l| l.starts_with("Memory"))
            .expect("memory header");
        assert!(lines[memory + 1].contains("Claude Code"));
        assert!(lines[memory + 1].contains('≥'));
        assert!(lines
            .iter()
            .any(|l| l.contains("likely compression and swap")));
        for line in &lines {
            assert!(columns(line) <= WIDTH, "{line}");
        }
        assert!(
            lines.len() <= MOST_LINES,
            "{} lines:\n{}",
            lines.len(),
            lines.join("\n")
        );
    }
}
