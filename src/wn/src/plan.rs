//! Reading a plan of parallel work out of a page of text.
//!
//! A chain says one order: do these issues, one after the other. A plan says
//! several orders side by side, one for each part of the repository that a
//! reader can work in without walking into the work of a neighbor. Such a plan
//! is written as a record for each stream, or as one table row for each stream.
//! This module reads both, and it gives back the same streams either way.
//!
//! # Why only the `Order` field holds a chain
//!
//! A stream carries prose as well as a chain. The prose is about code, and
//! prose about code is full of numbers: `main.rs:1566-1650` names two lines of
//! a file, and `265 lines apart in a 5113-line file` names a distance and a
//! length. None of them is an issue.
//!
//! So this module reads the `Order` field and it reads nothing else. `Stream`,
//! `Zone`, and `Notes` never give a number to a chain. A reader who writes a
//! note about line 5113 gets the issues of the plan, and not issue 5113.
//!
//! # The pair
//!
//! A step of a plan is one piece of work, and one piece of work is sometimes
//! two numbers: `PR#344 (#341)` is a pull request that closes an issue. The
//! step holds both, because the state of the work is the state of the pull
//! request and the reader still wants to see which issue it finishes.

use thiserror::Error;

use crate::chain::{read_number, IssueNumber, Snippet, SEPARATORS};

/// The character that marks a number as an issue number.
///
/// The same mark as the one the chain reader uses. It stands here as well
/// because a step ends where the next `#` starts, so `#1#2` is two steps
/// written with no separator at all.
const HASH: char = '#';

/// The character that opens the group of a pair.
const GROUP_OPEN: char = '(';

/// The character that closes the group of a pair.
const GROUP_CLOSE: char = ')';

/// The prefix a plan writes before the number of a pull request.
///
/// It carries no meaning for this module, because GitHub numbers a pull
/// request out of the same series as an issue. It is read and dropped so a
/// plan written the way a reader reads it is a plan this module reads too.
const PULL_REQUEST_PREFIX: &str = "pr";

/// The word a stream with no `Stream` field takes as the first half of its
/// label. The second half is the place of the stream in the plan.
const UNNAMED_LABEL: &str = "Stream";

/// The lowest number of characters a line of rule holds.
///
/// Two characters are an arrow (`--`) or the start of a word. Three are a rule.
const RULE_CHARS: usize = 3;

/// One piece of work of a stream.
///
/// A step is one number, and sometimes two: a pull request and the issue that
/// pull request closes. The two travel together because a reader who reads
/// `PR#344 (#341)` wants one row and not two, and because the state of the row
/// is the state of the pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The number of the work itself: the pull request of a pair.
    number: IssueNumber,
    /// The issue the work closes, when the plan names one.
    closes: Option<IssueNumber>,
}

impl Step {
    /// The step that does the work `number` names and closes `closes`.
    #[must_use]
    pub fn new(number: IssueNumber, closes: Option<IssueNumber>) -> Self {
        Self { number, closes }
    }

    /// The number of the work: the pull request of a pair, and the issue of a
    /// step that stands alone.
    #[must_use]
    pub fn number(&self) -> IssueNumber {
        self.number
    }

    /// The issue the work closes, or `None` when the plan names one number
    /// only.
    #[must_use]
    pub fn closes(&self) -> Option<IssueNumber> {
        self.closes
    }
}

/// One line of work of a plan, from its first step to its last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    /// The name the plan gives the stream, or the place of the stream in the
    /// plan when it gives none.
    label: String,
    /// The steps of the stream, in the order the plan writes them.
    steps: Vec<Step>,
}

impl Stream {
    /// The name of the stream, as the plan writes it.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The steps of the stream, in the order the plan writes them.
    #[must_use]
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }
}

/// Several streams of work, to walk side by side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The streams, in the order the plan writes them.
    streams: Vec<Stream>,
}

impl Plan {
    /// The streams of the plan, in the order the plan writes them.
    #[must_use]
    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    /// Every number of every stream, in the order of its first appearance.
    ///
    /// The number of a step comes before the number the step closes, because
    /// the pull request is the work and the issue is what the work finishes.
    /// A number that stands in two streams arrives once, so one query to
    /// GitHub answers the whole plan.
    #[must_use]
    pub fn numbers(&self) -> Vec<IssueNumber> {
        Vec::new()
    }
}

/// Why a page of text is not a plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    /// No stream of the text names a chain.
    #[error("the plan has no Order field. Each stream names its issues in one")]
    NoOrder,
    /// One stream of the plan names no chain.
    #[error("stream {0:?} has no Order field")]
    StreamWithoutOrder(Snippet),
    /// The `Order` field of a stream holds a token that names no issue.
    #[error("stream {stream:?}: {token:?} is not an issue number")]
    NotAnIssue {
        /// The label of the stream that holds the token.
        stream: Snippet,
        /// The token itself.
        token: Snippet,
    },
    /// The `Order` field of a stream holds no number at all.
    #[error("stream {0:?}: the Order field holds no issue number")]
    NoIssues(Snippet),
    /// A group stands before the first step of a stream, so it attaches to
    /// nothing.
    #[error("stream {stream:?}: {token:?} stands before any issue number")]
    UnattachedPair {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group itself.
        token: Snippet,
    },
    /// A second group stands on one step, and a step closes one issue.
    #[error("stream {stream:?}: {token:?} is a second issue for one step")]
    SecondPair {
        /// The label of the stream that holds the group.
        stream: Snippet,
        /// The group itself.
        token: Snippet,
    },
}

/// The four fields a stream of a plan is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    /// The name of the stream.
    Stream,
    /// The chain of the stream. The one field this module reads for numbers.
    Order,
    /// The part of the repository the stream works in.
    Zone,
    /// The prose of the stream.
    Notes,
}

impl Key {
    /// The four keys, to read a line against.
    const ALL: [Self; 4] = [Self::Stream, Self::Order, Self::Zone, Self::Notes];

    /// The word the key is written with.
    fn word(self) -> &'static str {
        match self {
            Self::Stream => "Stream",
            Self::Order => "Order",
            Self::Zone => "Zone",
            Self::Notes => "Notes",
        }
    }
}

/// Is `text` a plan of several streams, and not one chain?
///
/// True when a line of `text` opens a `Stream` field or an `Order` field, and
/// true when a row of a table names a `Stream` column or an `Order` column.
///
/// `Stream` counts on its own so that a plan with no `Order` field reaches
/// [`parse`] and earns an error that says which field is missing. The chain
/// reader would answer such a text with a complaint about the token `Stream:`,
/// which tells the reader nothing about what to write instead.
#[must_use]
pub fn looks_like_a_plan(text: &str) -> bool {
    let _ = text;
    false
}

/// Read the streams of `text`, in the order it writes them.
///
/// # Errors
///
/// Gives [`PlanError::NoOrder`] for a text where no stream names a chain,
/// [`PlanError::StreamWithoutOrder`] for one stream of such a text,
/// [`PlanError::NoIssues`] for an `Order` field with no number in it,
/// [`PlanError::NotAnIssue`] for a token of an `Order` field that names no
/// issue, and [`PlanError::UnattachedPair`] or [`PlanError::SecondPair`] for a
/// group that attaches to no step or to a step that already holds one.
pub fn parse(text: &str) -> Result<Plan, PlanError> {
    let _ = text;
    Ok(Plan {
        streams: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plan of issue #413, as a record for each stream.
    ///
    /// Real prose, because the trap this module exists for is real prose: the
    /// notes of three of these streams name numbers that are not issues.
    const RECORDS: &str = "\
Stream: S1 gitscratch → grind → grime
Order: PR#344 (#341) → PR#343 (#329) → PR#342 (#328) → #330 → #331
Zone: src/gitscratch, src/grist, new src/grind, src/grime
Notes: Three sibling PRs off one merge base. All three edit tests/safety.rs and both READMEs, so each merge
forces a rebase of the next. #341 is the bug (a rebase halt with nothing unmerged drops work), so it goes
ahead of the grind consumer. PR#342 pays the largest rebase.
────────────────────────────────────────
Stream: S2 ic
Order: #350 → #187 → #188
Zone: src/ic, src/termgfx
Notes: All three land inside display_image (main.rs:1566-1650). Highest collision in the set. Branch
ic-xtermjs already holds 5 commits of #188 and is dirty.
────────────────────────────────────────
Stream: S3 crap
Order: #314 → #315
Zone: src/crap
Notes: The two hunks sit 265 lines apart in a 5113-line file, so the rebase is cheap.
────────────────────────────────────────
Stream: S4 prcp
Order: #265 → #266
Zone: src/prcp
Notes: #320 landed 2026-08-25 and took the shell integration with it.
────────────────────────────────────────
Stream: S5 tvfind
Order: #321
Zone: src/tvfind
Notes: One issue, no neighbors.
────────────────────────────────────────
Stream: S6 vpn-tunnel
Order: #191 → #192
Zone: src/vpn-tunnel
Notes: Both edits land within a 30-line window of compose.rs.
────────────────────────────────────────
Stream: S7 dwt
Order: #196
Zone: src/dwt
Notes: Independent of everything above.";

    /// The same seven streams, as one table.
    ///
    /// The same labels and the same `Order` cells as [`RECORDS`], and notes
    /// that carry the same traps, so the two forms are asked for one answer.
    const TABLE: &str = "\
| Stream | Order | Zone | Notes |
| --- | --- | --- | --- |
| S1 gitscratch → grind → grime | PR#344 (#341) → PR#343 (#329) → PR#342 (#328) → #330 → #331 | src/gitscratch, src/grist, new src/grind, src/grime | Three sibling PRs off one merge base. #341 is the bug. PR#342 pays the largest rebase. |
| S2 ic | #350 → #187 → #188 | src/ic, src/termgfx | All three land inside display_image (main.rs:1566-1650). Branch ic-xtermjs holds 5 commits of #188. |
| S3 crap | #314 → #315 | src/crap | The two hunks sit 265 lines apart in a 5113-line file, so the rebase is cheap. |
| S4 prcp | #265 → #266 | src/prcp | #320 landed 2026-08-25 and took the shell integration with it. |
| S5 tvfind | #321 | src/tvfind | One issue, no neighbors. |
| S6 vpn-tunnel | #191 → #192 | src/vpn-tunnel | Both edits land within a 30-line window of compose.rs. |
| S7 dwt | #196 | src/dwt | Independent of everything above. |";

    /// The label of every stream of `plan`, and the numbers of every step.
    fn shape(plan: &Plan) -> Vec<(&str, Vec<(u64, Option<u64>)>)> {
        plan.streams()
            .iter()
            .map(|stream| (stream.label(), steps_of(stream)))
            .collect()
    }

    /// The numbers of every step of `stream`, the pair second.
    fn steps_of(stream: &Stream) -> Vec<(u64, Option<u64>)> {
        stream
            .steps()
            .iter()
            .map(|step| (step.number().get(), step.closes().map(IssueNumber::get)))
            .collect()
    }

    /// The numbers of every step of the stream at `index`.
    fn steps_at(plan: &Plan, index: usize) -> Vec<(u64, Option<u64>)> {
        steps_of(
            plan.streams()
                .get(index)
                .expect("the plan holds this stream"),
        )
    }

    /// The plan `text` writes.
    fn plan_of(text: &str) -> Plan {
        parse(text).expect("the text is a plan")
    }

    /// The numbers of `plan`, as a reader writes them.
    fn numbers_of(plan: &Plan) -> Vec<u64> {
        plan.numbers().iter().map(|number| number.get()).collect()
    }

    #[test]
    fn reads_a_record_for_each_stream_of_the_plan() {
        assert_eq!(
            shape(&plan_of(RECORDS)),
            vec![
                (
                    "S1 gitscratch → grind → grime",
                    vec![
                        (344, Some(341)),
                        (343, Some(329)),
                        (342, Some(328)),
                        (330, None),
                        (331, None),
                    ],
                ),
                ("S2 ic", vec![(350, None), (187, None), (188, None)]),
                ("S3 crap", vec![(314, None), (315, None)]),
                ("S4 prcp", vec![(265, None), (266, None)]),
                ("S5 tvfind", vec![(321, None)]),
                ("S6 vpn-tunnel", vec![(191, None), (192, None)]),
                ("S7 dwt", vec![(196, None)]),
            ]
        );
    }

    #[test]
    fn the_notes_of_a_stream_give_no_number_to_its_chain() {
        let plan = plan_of(RECORDS);
        assert_eq!(
            steps_at(&plan, 2),
            vec![(314, None), (315, None)],
            "the notes of S3 name 265 and 5113, which a reader that hunts numbers takes for issues"
        );
        assert_eq!(
            steps_at(&plan, 0).len(),
            5,
            "the notes of S1 name #341 and PR#342"
        );
        assert_eq!(
            steps_at(&plan, 1).len(),
            3,
            "the notes of S2 name main.rs:1566-1650, 5 commits, and #188"
        );
    }

    #[test]
    fn a_notes_field_of_three_lines_gives_no_number_to_the_chain() {
        // The second line of the notes of S1 holds #341 and the third holds
        // PR#342. A reader of continuation lines takes each of them a second
        // time, which writes the same issue into the chain twice.
        let steps = steps_at(&plan_of(RECORDS), 0);
        assert_eq!(steps.len(), 5);
        let numbers: Vec<u64> = steps
            .iter()
            .flat_map(|(number, closes)| [Some(*number), *closes])
            .flatten()
            .collect();
        let mut once = numbers.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(once.len(), numbers.len(), "{numbers:?}");
    }

    #[test]
    fn a_rule_between_two_records_is_not_a_stream() {
        let no_rules: String = RECORDS
            .lines()
            .filter(|line| !line.starts_with('\u{2500}'))
            .collect::<Vec<_>>()
            .join("\n");
        let plan = plan_of(&no_rules);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(RECORDS));
    }

    #[test]
    fn a_record_with_no_stream_field_takes_its_place_as_a_label() {
        let no_names: String = RECORDS
            .lines()
            .filter(|line| !line.starts_with("Stream:"))
            .collect::<Vec<_>>()
            .join("\n");
        let named = plan_of(RECORDS);
        let unnamed = plan_of(&no_names);
        assert_eq!(
            unnamed
                .streams()
                .iter()
                .map(Stream::label)
                .collect::<Vec<_>>(),
            vec![
                "Stream 1", "Stream 2", "Stream 3", "Stream 4", "Stream 5", "Stream 6", "Stream 7",
            ]
        );
        assert_eq!(
            unnamed.streams().iter().map(steps_of).collect::<Vec<_>>(),
            named.streams().iter().map(steps_of).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_table_form_gives_the_streams_of_the_record_form() {
        let plan = plan_of(TABLE);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(RECORDS));
    }

    #[test]
    fn a_table_with_no_outer_bars_gives_the_same_streams() {
        let bare: String = TABLE
            .lines()
            .map(|line| {
                line.trim()
                    .trim_start_matches('|')
                    .trim_end_matches('|')
                    .trim()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let plan = plan_of(&bare);
        assert_eq!(plan.streams().len(), 7);
        assert_eq!(plan, plan_of(TABLE));
    }

    #[test]
    fn a_group_names_the_issue_the_step_before_it_closes() {
        let steps = steps_at(&plan_of("Order: PR#344 (#341) → #330"), 0);
        assert_eq!(steps, vec![(344, Some(341)), (330, None)]);
    }

    #[test]
    fn a_pr_prefix_is_read_whatever_its_case_and_a_bare_number_is_read() {
        assert_eq!(
            steps_at(
                &plan_of("Order: pr#344 (#341) → Pr#343 → PR342 → 330 → #331"),
                0
            ),
            vec![
                (344, Some(341)),
                (343, None),
                (342, None),
                (330, None),
                (331, None),
            ]
        );
    }

    #[test]
    fn a_key_is_read_whatever_its_case() {
        let plan = plan_of("stream: S1\nORDER: #1 → #2");
        assert_eq!(steps_at(&plan, 0), vec![(1, None), (2, None)]);
        assert_eq!(plan, plan_of("Stream: S1\nOrder: #1 → #2"));
    }

    #[test]
    fn an_order_field_of_more_than_one_line_joins_with_one_space() {
        assert_eq!(
            steps_at(&plan_of("Stream: S1\nOrder: #1 → #2 →\n#3"), 0),
            vec![(1, None), (2, None), (3, None)]
        );
    }

    #[test]
    fn a_word_with_a_colon_inside_prose_is_not_a_key() {
        // A key stands first or it is not a key. The word here opens no field,
        // so the line continues the notes and gives no number to the chain.
        let plan = plan_of(
            "Stream: S1\nOrder: #1\nNotes: the plan of a day\nFinish-what-we-started: #99 first",
        );
        assert_eq!(steps_at(&plan, 0), vec![(1, None)]);
    }

    #[test]
    fn an_empty_line_closes_the_field_it_holds_open() {
        // Text after an empty line that opens no field is loose prose, and
        // loose prose is not part of the chain above it.
        let plan = plan_of("Stream: S1\nOrder: #1 → #2\n\n#3 and #4 are notes to myself");
        assert_eq!(steps_at(&plan, 0), vec![(1, None), (2, None)]);
    }

    #[test]
    fn every_number_of_every_stream_arrives_once() {
        assert_eq!(
            numbers_of(&plan_of(RECORDS)),
            vec![
                344, 341, 343, 329, 342, 328, 330, 331, 350, 187, 188, 314, 315, 265, 266, 321,
                191, 192, 196,
            ]
        );
        assert_eq!(
            numbers_of(&plan_of(
                "Stream: A\nOrder: PR#344 (#341) → #330\nStream: B\nOrder: #330 → #341"
            )),
            vec![344, 341, 330],
            "one number of two streams is one number to ask GitHub about"
        );
    }

    #[test]
    fn a_chain_is_not_a_plan() {
        assert!(!looks_like_a_plan("#277 → #278 ∥ #279"));
        assert!(!looks_like_a_plan("#1 || #2"));
        assert!(!looks_like_a_plan(""));
    }

    #[test]
    fn a_stream_field_or_an_order_field_makes_a_plan() {
        assert!(looks_like_a_plan(RECORDS));
        assert!(looks_like_a_plan(TABLE));
        assert!(looks_like_a_plan("Order: #1 → #2"));
        assert!(looks_like_a_plan("  stream: S1"));
        assert!(looks_like_a_plan("| Stream | Order |"));
    }

    #[test]
    fn refuses_a_token_of_an_order_field_that_is_not_a_number() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #277 an #278"),
            Err(PlanError::NotAnIssue {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("an"),
            })
        );
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #277 an #278")
                .expect_err("the word is not an issue number")
                .to_string(),
            "stream \"S1 ic\": \"an\" is not an issue number"
        );
    }

    #[test]
    fn refuses_a_plan_that_holds_no_order_field() {
        let message = parse("Stream: S1 ic\nZone: src/ic\nStream: S2 crap\nZone: src/crap")
            .expect_err("a plan with no chain names nothing to do")
            .to_string();
        assert!(message.contains("no Order field"), "{message}");
        assert_eq!(
            parse("Stream: S1 ic\nZone: src/ic"),
            Err(PlanError::NoOrder)
        );
    }

    #[test]
    fn refuses_one_stream_that_holds_no_order_field() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: #350\nStream: S2 crap\nZone: src/crap"),
            Err(PlanError::StreamWithoutOrder(Snippet::new("S2 crap")))
        );
    }

    #[test]
    fn refuses_an_order_field_that_holds_no_number() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder:\nZone: src/ic"),
            Err(PlanError::NoIssues(Snippet::new("S1 ic")))
        );
    }

    #[test]
    fn refuses_a_group_that_stands_before_every_step() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: (#341) → #330"),
            Err(PlanError::UnattachedPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#341)"),
            })
        );
    }

    #[test]
    fn refuses_a_second_group_on_one_step() {
        assert_eq!(
            parse("Stream: S1 ic\nOrder: PR#344 (#341) (#329)"),
            Err(PlanError::SecondPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#329)"),
            })
        );
        assert_eq!(
            parse("Stream: S1 ic\nOrder: PR#344 (#341 #329)"),
            Err(PlanError::SecondPair {
                stream: Snippet::new("S1 ic"),
                token: Snippet::new("(#329)"),
            })
        );
    }

    #[test]
    fn a_long_stream_label_is_cut_in_the_message() {
        // A plan arrives from the clipboard, and a clipboard holds a page of
        // prose as readily as a label. A message that repeats the whole page
        // hides its own last line.
        let label = "a".repeat(200);
        let message = parse(&format!("Stream: {label}\nOrder: an"))
            .expect_err("the word is not an issue number")
            .to_string();
        let cut: String = label.chars().take(crate::chain::SNIPPET_CHARS).collect();
        assert!(!message.contains(&label), "{message}");
        assert!(message.contains(&format!("\"{cut}…\"")), "{message}");
    }
}
