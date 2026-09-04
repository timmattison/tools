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

use std::path::Path;

use unicode_width::UnicodeWidthStr;

use crate::metrics::Uncommitted;
use crate::scratch::Conflicts;

/// Width of the `": "` between the tool name and the verdict on the first line.
///
/// The summary sits on a second line indented to line up with the text after
/// it, so the indent has to be measured from the tool's own name rather than
/// written out as a fixed run of spaces that only happens to fit today.
const LABEL_SEPARATOR_WIDTH: usize = 2;

/// Indent for every line of the per-file breakdown.
const FILE_INDENT: &str = "  ";

/// Columns between the name column and the hunk counts, so the counts form a
/// column of their own instead of butting up against the longest name.
///
/// The name column is as wide as the widest name that fits the terminal, which
/// on a terminal wide enough is the widest name there is. A name wider than
/// that takes a row of its own - see [`Report::render_within`].
const COUNT_GAP: usize = 4;

/// A conflict verdict, and how to word it for one particular tool.
///
/// Three `Copy` fields, so the type is `Copy` too - which is what lets every
/// builder method take `self` without the value it was called on being spent.
/// `Debug` is here for the reason the API guidelines give: a consumer holding
/// one has to be able to derive `Debug` on the struct that holds it, and an
/// assertion that fails while comparing two renderings has to be able to say
/// which report produced them.
#[derive(Debug, Clone, Copy)]
pub struct Report<'a> {
    tool: &'a str,
    action: &'a str,
    show_stops: bool,
}

impl<'a> Report<'a> {
    /// Begin wording a verdict for `tool`.
    ///
    /// `tool` is the binary's own name - `"grind"` - because every line the
    /// tool prints is prefixed with it, the way a well-behaved unix tool
    /// identifies itself in a pipeline.
    ///
    /// The other half of the sentence arrives through [`describing`] rather
    /// than as a second argument here, and the split is the whole point: two
    /// adjacent `&str` parameters can be handed over the wrong way round in
    /// perfect silence, and the result is not a compile error but a report
    /// prefixed with `"replaying HEAD onto main"` and indented to the width of
    /// it. Named calls cannot be transposed, so the mistake stops being
    /// available rather than merely being documented against.
    ///
    /// A report that is never given an action words itself with an empty one -
    /// visibly wrong at a glance, and reachable only by not finishing the
    /// sentence, which no call site does.
    ///
    /// [`describing`]: Report::describing
    #[must_use]
    pub fn for_tool(tool: &'a str) -> Self {
        Self {
            tool,
            action: "",
            show_stops: true,
        }
    }

    /// Say what was replayed.
    ///
    /// `action` is a present participle phrase - `"replaying HEAD onto main"`,
    /// `"merging feature into HEAD"` - so it reads correctly in both the clean
    /// sentence and the conflict header, which are the only two places it
    /// lands.
    #[must_use]
    pub fn describing(self, action: &'a str) -> Self {
        Self { action, ..self }
    }

    /// Drop the stop count from the summary.
    ///
    /// A merge halts exactly once, so the number carries no information for
    /// `grime` and printing it would invite a reader to compare a constant
    /// against `grind`'s real measurement. [`Conflicts`] still records it; this
    /// only decides whether it is worth saying out loud.
    ///
    /// Takes `self` and leaves the original usable, because [`Report`] is
    /// `Copy`: a caller wanting both wordings of the same verdict gets them
    /// without building the report twice.
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
    ///
    /// This is the unbounded layout: the count column goes wherever the widest
    /// name puts it. That is right for a caller writing to a file, to a pipe or
    /// to an assertion, all of which have no right-hand edge. A caller writing
    /// to a terminal has one, and reaches for [`Report::render_within`].
    #[must_use]
    pub fn render(&self, conflicts: &Conflicts) -> String {
        self.render_within(conflicts, usize::MAX)
    }

    /// The same verdict, laid out for a terminal `columns` columns wide.
    ///
    /// The counts share one column, and the whole reason to measure a name in
    /// display width is so that column is the same one on every row. Past the
    /// right-hand edge of the terminal there is no such column: the terminal
    /// wraps each of those rows at a point of its own choosing, and the result
    /// reads worse than a breakdown nobody tried to align. One deeply nested
    /// path is enough to do it, and a deeply nested path is ordinary.
    ///
    /// So the name column is clamped to what `columns` leaves after the indent,
    /// the gap and the widest count. A name too wide for the clamp takes a row
    /// of its own and its count takes the next row, still in the shared column,
    /// so an over-long name costs one extra row rather than the alignment of
    /// every row. The name itself is never cut short, because a truncated path
    /// opens no file.
    ///
    /// Nothing here reads the terminal. The number arrives as a parameter so
    /// the library stays a library, and each binary answers the question with
    /// `termbar::TerminalWidth::get_or_default`.
    #[must_use]
    pub fn render_within(&self, conflicts: &Conflicts, columns: usize) -> String {
        if conflicts.is_clean() {
            return format!("{}: clean - {} hit no conflicts", self.tool, self.action);
        }

        // Everything below is padded in display width, never in bytes or
        // characters. A path can be any of the three lengths at once - a CJK
        // name is one character and three bytes per glyph but two terminal
        // columns - and only the third is what a reader sees line up.
        //
        // `printable` is the one place a name stops being the bytes git
        // reported, and it is here rather than anywhere upstream on purpose: a
        // terminal can only be handed text, so a name that is not valid UTF-8
        // has to become U+FFFD to be printed at all, and a name holding a
        // control character has to have that character spelled out. Converting
        // it any earlier would put that name back into the map, where it names
        // no file and costs the file its real hunk count. Each name is
        // converted once, here, and the same converted string is both measured
        // and printed - so the column a reader sees is the column the padding
        // was computed for, and no name can measure one width and print
        // another.
        let indent = " ".repeat(self.tool.width() + LABEL_SEPARATOR_WIDTH);
        let rows: Vec<(String, String)> = conflicts
            .file_hunks()
            .map(|(name, hunks)| (printable(name), hunks.phrase()))
            .collect();

        // What the terminal leaves for a name: everything but the indent, the
        // gap and the widest count. `saturating_sub` because a terminal narrow
        // enough to leave nothing is still a terminal, and it gets every name
        // on a row of its own.
        let widest_count = rows
            .iter()
            .map(|(_, count)| count.width())
            .max()
            .unwrap_or_default();
        let room = columns.saturating_sub(FILE_INDENT.width() + COUNT_GAP + widest_count);
        let widest = rows
            .iter()
            .map(|(name, _)| name.width())
            .max()
            .unwrap_or_default()
            .min(room);
        let count_column = " ".repeat(FILE_INDENT.width() + widest + COUNT_GAP);

        let mut summary = format!(
            "{indent}{} across {}",
            conflicts.hunks().phrase(),
            conflicts.files().phrase()
        );
        if self.show_stops {
            summary.push_str(&format!(", {}", conflicts.stops().phrase()));
        }

        let mut lines = vec![
            format!("{}: conflicts - {}", self.tool, self.action),
            summary,
            // The breakdown is a separate thought from the summary, and a blank
            // line is how a terminal says so.
            String::new(),
        ];

        for (name, count) in &rows {
            if name.width() > widest {
                lines.push(format!("{FILE_INDENT}{name}"));
                lines.push(format!("{count_column}{count}"));
            } else {
                let gap = " ".repeat(widest - name.width() + COUNT_GAP);
                lines.push(format!("{FILE_INDENT}{name}{gap}{count}"));
            }
        }

        lines.join("\n")
    }

    /// The stderr note warning that uncommitted work is not covered, or `None`
    /// when there is none to warn about.
    ///
    /// A replay only ever sees committed work, so a `clean` verdict on a dirty
    /// tree is true and still misleading. The note exists so it cannot be
    /// misread.
    ///
    /// `None` rather than an empty string, so a caller cannot print a blank
    /// line for a tree that had nothing worth warning about: there either is a
    /// note or there is not.
    #[must_use]
    pub fn dirty_note(&self, uncommitted: Uncommitted) -> Option<String> {
        if uncommitted == Uncommitted::new(0) {
            return None;
        }

        // The noun and its plural are [`Uncommitted`]'s business, so all that
        // is left here is the verb that has to agree with the number the
        // counter is about to word.
        let verb = if uncommitted == Uncommitted::new(1) {
            "is"
        } else {
            "are"
        };

        Some(format!(
            "{}: note: {} {verb} not included; simulating from HEAD",
            self.tool,
            uncommitted.phrase()
        ))
    }
}

/// `name` as text a terminal can be handed, with every control character
/// spelled out.
///
/// Two losses happen here and nowhere earlier. The first is the lossy decode: a
/// name that is not valid UTF-8 becomes U+FFFD, because a terminal takes text
/// and nothing else. The second is this escape: NUL is the only byte a path
/// cannot hold, which is the premise the `-z` reader rests on, so a newline, a
/// carriage return and an ESC are all legal in a name and none of them can be
/// printed as itself. A newline splits one row of a line-oriented layout in
/// two. An ESC hands an escape sequence out of the repository straight to the
/// terminal of whoever ran the tool. And `unicode-width` measures every one of
/// them as no columns at all, so a name holding one is measured shorter than it
/// prints and moves the count column of every other row with it.
///
/// The rule is [`char::is_control`] and nothing wider, which is what keeps this
/// from undoing the names the crate went to trouble to carry intact. A leading
/// space, a trailing space and U+3000 IDEOGRAPHIC SPACE are all printable
/// characters, so all three arrive unchanged - and so do a backslash, a double
/// quote and an emoji, each of which [`char::escape_debug`] would have escaped
/// into a name that names no file. `escape_debug` would take U+3000 as well:
/// it is a space separator rather than a printable character, by the table that
/// function reads.
///
/// Every control character comes out in the one form, `\u{...}`, rather than in
/// the short spellings `\n` and `\t`. The short forms read more easily and are
/// the more ambiguous of the two: a path really can hold a backslash followed
/// by an `n`, and this escape leaves that backslash alone, so `\n` on screen
/// would name two different files. `\u{a}` names one.
fn printable(name: &Path) -> String {
    let text = name.to_string_lossy();
    if !text.contains(char::is_control) {
        return text.into_owned();
    }

    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            escaped.push_str(&format!("\\u{{{:x}}}", character as u32));
        } else {
            escaped.push(character);
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::path::PathBuf;

    use unicode_width::UnicodeWidthStr;

    use super::{Report, FILE_INDENT};
    use crate::metrics::{Stops, Uncommitted};
    use crate::scratch::Conflicts;

    /// Which terminal column the hunk count starts in, measured in display
    /// width rather than bytes or characters - which is the only measure a
    /// reader looking at aligned columns can see.
    fn count_column(line: &str, count: &str) -> usize {
        line.split(count)
            .next()
            .expect("splitting always yields at least one piece")
            .width()
    }

    /// The per-file rows, which are everything after the blank line that
    /// separates them from the summary.
    ///
    /// Counted rather than searched for, so a name that split its own row in
    /// two is a row count this suite can see.
    fn breakdown(rendered: &str) -> Vec<&str> {
        rendered
            .lines()
            .skip_while(|line| !line.is_empty())
            .skip(1)
            .collect()
    }

    /// The one rendered row that carries `count`.
    ///
    /// # Panics
    ///
    /// Panics when no row carries it, because a count the renderer never
    /// printed is a failure of the renderer and not of the search.
    fn row_holding<'a>(rendered: &'a str, count: &str) -> &'a str {
        rendered
            .lines()
            .find(|line| line.contains(count))
            .unwrap_or_else(|| panic!("no rendered row carries {count:?}:\n{rendered}"))
    }

    /// One entry of a per-file breakdown.
    ///
    /// The count is a [`NonZeroUsize`] in the constructor, because a file that
    /// conflicted cost at least one decision, so every fixture goes through
    /// this rather than repeating the wrap at each call site.
    fn file(name: &str, hunks: usize) -> (PathBuf, NonZeroUsize) {
        (
            PathBuf::from(name),
            NonZeroUsize::new(hunks).expect("a conflicted file contributes at least one hunk"),
        )
    }

    /// The two-file, four-hunk, three-stop result the design spec's sample
    /// output describes, so every rendering test is measured against the shape
    /// `grind` and `grime` promised to print.
    fn sample() -> Conflicts {
        Conflicts::from_files(
            [file("src/lib.rs", 3), file("src/main.rs", 1)],
            Stops::new(3),
        )
    }

    #[test]
    fn a_clean_replay_gets_one_line_naming_the_tool_and_what_it_tried() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto origin/main");

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
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");

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
        let report = Report::for_tool("grime")
            .describing("merging feature into HEAD")
            .without_stops();

        assert_eq!(
            report.render(&sample()),
            r"grime: conflicts - merging feature into HEAD
       4 hunks across 2 files

  src/lib.rs     3 hunks
  src/main.rs    1 hunk"
        );
    }

    /// Both derives, exercised the way a consumer would.
    ///
    /// This one is a compile-time guarantee wearing a test's clothes, and it is
    /// worth being plain about that: drop `Copy` and the second use of `report`
    /// is a use-after-move, drop `Debug` and the format string does not build.
    /// Neither failure is an assertion that goes red - the crate simply stops
    /// compiling - so what this really does is write the requirement down
    /// somewhere that a person deleting a derive will trip over it.
    ///
    /// The assertions themselves are the part that *can* fail at runtime: a
    /// hand-written `Debug` that named neither the tool nor the action would
    /// satisfy the trait and still be useless in the assertion message the
    /// guideline exists to serve.
    #[test]
    fn a_report_can_be_debugged_and_used_again_after_being_narrowed() {
        let report = Report::for_tool("grime").describing("merging feature into HEAD");

        // `without_stops` consumes a copy, so the report it was called on is
        // still there - and still says everything it said before.
        let brief = report.without_stops();
        assert!(report.render(&sample()).contains("3 stops"));
        assert!(!brief.render(&sample()).contains("stops"));

        let debugged = format!("{report:?}");
        for expected in ["grime", "merging feature into HEAD"] {
            assert!(
                debugged.contains(expected),
                "a debug representation that omits {expected:?} tells a failing \
                 assertion nothing: {debugged}"
            );
        }
    }

    /// The smallest possible conflict is also the most common one, and "1 hunks
    /// across 1 files, 1 stops" is the tell that nobody looked at the output
    /// before shipping it.
    #[test]
    fn a_single_hunk_in_a_single_file_reads_in_the_singular_throughout() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");
        let one = Conflicts::from_files([file("src/lib.rs", 1)], Stops::new(1));

        assert_eq!(
            report.render(&one),
            r"grind: conflicts - replaying HEAD onto main
       1 hunk across 1 file, 1 stop

  src/lib.rs    1 hunk"
        );
    }

    /// A CJK file name occupies two terminal columns per character while being
    /// one character - and three bytes - wide, so padding by either of the two
    /// numbers Rust hands you for free produces a ragged column. Only display
    /// width lines these up for the person actually reading them.
    #[test]
    fn the_hunk_counts_line_up_by_display_width_not_by_character_count() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");
        // `日本語.txt` is 7 characters and 13 bytes, but 10 columns wide - one
        // wider than `readme.md`, which is 9 of all three.
        let wide =
            Conflicts::from_files([file("readme.md", 2), file("日本語.txt", 1)], Stops::new(2));

        let rendered = report.render(&wide);

        assert_eq!(
            rendered,
            r"grind: conflicts - replaying HEAD onto main
       3 hunks across 2 files, 2 stops

  readme.md     2 hunks
  日本語.txt    1 hunk"
        );

        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(
            count_column(lines[3], "2 hunks"),
            count_column(lines[4], "1 hunk"),
            "the counts should start in the same terminal column:\n{rendered}"
        );
    }

    /// A file name holds every byte but NUL, which is the premise the `-z`
    /// reader rests on, so a newline, a carriage return and an ESC are all
    /// legal in one. This layout is line-oriented and column-aligned, and a raw
    /// control character wrecks it three ways at once.
    ///
    /// A newline splits one row in two and leaves the count stranded on the
    /// second. An ESC hands an escape sequence out of the repository straight
    /// to the terminal of whoever ran the tool. And `unicode-width` measures
    /// every control character as no columns at all, so the padding and the
    /// terminal disagree about where the count column is - on every other row
    /// as well, because one over-measured name sets the width of all of them.
    ///
    /// The escape happens once, ahead of the measurement and ahead of the
    /// print, so the string that was measured is the string that reaches the
    /// screen.
    #[test]
    fn a_control_character_in_a_name_is_escaped_rather_than_printed_raw() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");
        let controlled = Conflicts::from_files(
            [
                file("plain.rs", 1),
                file("src/esc\u{1b}[31m.rs", 1),
                file("src/two\nlines.rs", 2),
            ],
            Stops::new(2),
        );

        let rendered = report.render(&controlled);
        let rows = breakdown(&rendered);

        assert_eq!(
            rows.len(),
            3,
            "three conflicted files are three rows, whatever their names hold:\n{rendered}"
        );
        assert!(
            !rendered.contains('\u{1b}'),
            "an ESC out of a repository must never reach a terminal: {rendered:?}"
        );
        assert!(
            rendered.contains(r"src/two\u{a}lines.rs"),
            "a newline is escaped in place, so the name stays one readable row:\n{rendered}"
        );
        assert!(
            rendered.contains(r"src/esc\u{1b}[31m.rs"),
            "an ESC is escaped in place, so the name stays readable:\n{rendered}"
        );

        let columns: Vec<usize> = [
            (rows[0], "1 hunk"),
            (rows[1], "1 hunk"),
            (rows[2], "2 hunks"),
        ]
        .into_iter()
        .map(|(row, count)| count_column(row, count))
        .collect();
        assert!(
            columns.iter().all(|column| *column == columns[0]),
            "every count starts in the same terminal column, and a control \
             character measured as nothing is what moves one of them: \
             {columns:?}\n{rendered}"
        );
    }

    /// The count column sits past the widest name, and nothing about a name is
    /// bounded - a deeply nested path is ordinary, and it carries the counts of
    /// every other row off the right-hand edge with it. The terminal then wraps
    /// each of those rows, and a wrapped column reads worse than a column
    /// nobody tried to align.
    ///
    /// So the caller says how many columns it has, and the name column is
    /// clamped to what is left after the indent, the gap and the widest count.
    /// A name too wide for that clamp takes a row of its own, and its count
    /// takes the next row, in the same column as every other count. The name is
    /// never cut short: a truncated path opens no file.
    #[test]
    fn a_name_too_wide_for_the_terminal_keeps_the_counts_on_screen_and_the_name_whole() {
        /// A narrow terminal, so one ordinary nested path is wider than it.
        const COLUMNS: usize = 40;
        /// 61 columns of perfectly ordinary path.
        const LONG: &str = "src/a/very/deeply/nested/directory/with/a/long/name/module.rs";

        let report = Report::for_tool("grind").describing("replaying HEAD onto main");
        let deep = Conflicts::from_files([file(LONG, 3), file("readme.md", 1)], Stops::new(2));

        let rendered = report.render_within(&deep, COLUMNS);

        assert!(
            rendered
                .lines()
                .any(|line| line == format!("{FILE_INDENT}{LONG}")),
            "a name too wide to pad takes a row of its own, whole - a path cut \
             short opens no file:\n{rendered}"
        );

        let short_row = row_holding(&rendered, "1 hunk");
        let long_row = row_holding(&rendered, "3 hunks");
        assert_eq!(
            count_column(short_row, "1 hunk"),
            count_column(long_row, "3 hunks"),
            "both counts still start in the same column:\n{rendered}"
        );
        for (row, count) in [(short_row, "1 hunk"), (long_row, "3 hunks")] {
            assert!(
                row.width() <= COLUMNS,
                "the row carrying {count} has to fit in {COLUMNS} columns and \
                 takes {}, so the terminal wraps it:\n{rendered}",
                row.width()
            );
        }
    }

    /// A clean tree has nothing to warn about, and warning anyway would train
    /// people to ignore the line that matters.
    ///
    /// Asserted through both spellings of nothing, because a default
    /// [`Uncommitted`] is what a caller that could not measure the tree falls
    /// back to, and falling back must mean "say nothing" rather than "say
    /// something about zero files".
    #[test]
    fn a_clean_tree_gets_no_dirty_note() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");

        assert_eq!(report.dirty_note(Uncommitted::new(0)), None);
        assert_eq!(report.dirty_note(Uncommitted::default()), None);
    }

    /// The note exists so a `clean` verdict is never misread as covering work
    /// that was never committed, which means it has to read like a sentence
    /// rather than like a counter - "1 uncommitted file is", not "1 files are".
    ///
    /// The noun comes from [`Uncommitted`] and only the verb is chosen here, so
    /// this pins the seam: the two halves of the sentence are written in two
    /// different places and still have to agree about the number.
    #[test]
    fn the_dirty_note_agrees_with_itself_about_how_many_files_there_are() {
        let report = Report::for_tool("grind").describing("replaying HEAD onto main");

        assert_eq!(
            report.dirty_note(Uncommitted::new(1)).as_deref(),
            Some("grind: note: 1 uncommitted file is not included; simulating from HEAD")
        );
        assert_eq!(
            report.dirty_note(Uncommitted::new(3)).as_deref(),
            Some("grind: note: 3 uncommitted files are not included; simulating from HEAD")
        );
    }
}
