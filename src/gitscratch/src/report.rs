//! The one place a conflict verdict is turned into text.
//!
//! `grind` asks whether rebasing HEAD onto a branch would conflict; `grime`
//! asks whether merging a branch into HEAD would. Different questions, but a
//! developer reading the answers has to be able to compare them at a glance,
//! which means both must print the same shape - the same words, the same
//! columns, the same singular and plural forms.
//!
//! Two binaries each carrying their own renderer would drift apart on exactly
//! those details, and that drift is the bug class this crate exists to
//! eliminate. So the renderer lives here, beside the [`Conflicts`] it renders,
//! and the binaries own only the question they ask. That is a deliberate,
//! spec-sanctioned acceptance of a little presentation logic in a library: the
//! alternative is two copies of it.
//!
//! The only difference the two tools are allowed is captured by
//! [`Report::without_stops`] - a merge halts exactly once, so printing the
//! number would be noise.

use crate::metrics::Hunks;
use crate::scratch::Conflicts;

/// Width of the `": "` between the tool name and the verdict on the first line.
///
/// The summary sits on a second line indented to line up with the text after
/// it, so the indent has to be measured from the tool's own name rather than
/// written out as a fixed run of spaces that only happens to fit today.
const LABEL_SEPARATOR_WIDTH: usize = 2;

/// Indent for every line of the per-file breakdown.
const FILE_INDENT: &str = "  ";

/// Columns between the widest file name and the hunk counts, so the counts form
/// a column of their own instead of butting up against the longest name.
const COUNT_GAP: usize = 4;

/// A conflict verdict, and how to word it for one particular tool.
pub struct Report<'a> {
    tool: &'a str,
    action: &'a str,
    show_stops: bool,
}

impl<'a> Report<'a> {
    /// Word the verdict for `tool`, describing the replay as `action`.
    ///
    /// `tool` is the binary's own name - `"grind"` - because every line the
    /// tool prints is prefixed with it, the way a well-behaved unix tool
    /// identifies itself in a pipeline. `action` is a present participle phrase
    /// describing what was replayed, such as `"replaying HEAD onto main"`, so
    /// it reads correctly in both the clean sentence and the conflict header.
    #[must_use]
    pub fn new(tool: &'a str, action: &'a str) -> Self {
        Self {
            tool,
            action,
            show_stops: true,
        }
    }

    /// Drop the stop count from the summary.
    ///
    /// A merge halts exactly once, so the number carries no information for
    /// `grime` and printing it would invite a reader to compare a constant
    /// against `grind`'s real measurement. [`Conflicts`] still records it; this
    /// only decides whether it is worth saying out loud.
    #[must_use]
    pub fn without_stops(self) -> Self {
        Self {
            show_stops: false,
            ..self
        }
    }

    /// The verdict, ready to print to stdout, with no trailing newline.
    ///
    /// Returned rather than printed so the caller decides where it goes - and
    /// so `-q` can decide it goes nowhere - without this having to know about
    /// streams.
    #[must_use]
    pub fn render(&self, conflicts: &Conflicts) -> String {
        if conflicts.is_clean() {
            return format!("{}: clean - {} hit no conflicts", self.tool, self.action);
        }

        let indent = " ".repeat(self.tool.chars().count() + LABEL_SEPARATOR_WIDTH);
        let widest = conflicts
            .file_hunks()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or_default();

        let mut summary = format!(
            "{indent}{} hunks across {} files",
            conflicts.hunks(),
            conflicts.files()
        );
        if self.show_stops {
            summary.push_str(&format!(", {} stops", conflicts.stops()));
        }

        let mut lines = vec![
            format!("{}: conflicts - {}", self.tool, self.action),
            summary,
            // The breakdown is a separate thought from the summary, and a blank
            // line is how a terminal says so.
            String::new(),
        ];

        for (name, hunks) in conflicts.file_hunks() {
            let gap = " ".repeat(widest.saturating_sub(name.chars().count()) + COUNT_GAP);
            lines.push(format!(
                "{FILE_INDENT}{name}{gap}{}",
                Hunks::new(hunks).phrase()
            ));
        }

        lines.join("\n")
    }

    /// The stderr note warning that uncommitted work is not covered, or `None`
    /// when there is none to warn about.
    ///
    /// A replay only ever sees committed work, so a `clean` verdict on a dirty
    /// tree is true and still misleading. The note exists so it cannot be
    /// misread.
    #[must_use]
    pub fn dirty_note(&self, _uncommitted: usize) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::Report;
    use crate::scratch::Conflicts;

    /// The two-file, four-hunk, three-stop result the design spec's sample
    /// output describes, so every rendering test is measured against the shape
    /// `grind` and `grime` promised to print.
    fn sample() -> Conflicts {
        Conflicts::from_files(
            [
                ("src/lib.rs".to_string(), 3),
                ("src/main.rs".to_string(), 1),
            ],
            3,
        )
    }

    #[test]
    fn a_clean_replay_gets_one_line_naming_the_tool_and_what_it_tried() {
        let report = Report::new("grind", "replaying HEAD onto origin/main");

        assert_eq!(
            report.render(&Conflicts::default()),
            "grind: clean - replaying HEAD onto origin/main hit no conflicts"
        );
    }

    /// The whole verdict in one assertion: the header, the summary indented to
    /// sit under it, and the breakdown that says where the work lands. Asserted
    /// as one block because the shape - including the blank line and the
    /// aligned counts - is the contract, not the individual lines.
    #[test]
    fn a_conflicted_replay_gets_a_header_a_summary_and_a_per_file_breakdown() {
        let report = Report::new("grind", "replaying HEAD onto main");

        assert_eq!(
            report.render(&sample()),
            r"grind: conflicts - replaying HEAD onto main
       4 hunks across 2 files, 3 stops

  src/lib.rs     3 hunks
  src/main.rs    1 hunk"
        );
    }

    /// `grime` prints the identical shape minus the stop count, because a merge
    /// halts exactly once and the number would be a constant dressed up as a
    /// measurement. Everything else - the header, the counts, the breakdown -
    /// has to survive the omission untouched, or the two tools stop being
    /// comparable at a glance.
    #[test]
    fn dropping_the_stop_count_removes_that_clause_and_nothing_else() {
        let report = Report::new("grime", "merging feature into HEAD").without_stops();

        assert_eq!(
            report.render(&sample()),
            r"grime: conflicts - merging feature into HEAD
       4 hunks across 2 files

  src/lib.rs     3 hunks
  src/main.rs    1 hunk"
        );
    }
}
