//! What an issue says comes before it.
//!
//! A plan is a claim about the order of the work, and the issues make a claim
//! of their own. The `to-issues` skill writes the blockers of a slice under a
//! heading, one to a list item:
//!
//! ```text
//! ## Blocked by
//!
//! - #168
//! ```
//!
//! A plan once put #170 before #168, and #170 said that #168 blocked it. `wn`
//! answered the plan and sent the reader to #170. This module reads that claim
//! out of the body of an issue, so the answer can hold the plan to it.
//!
//! # A heading and a list item are blocks, not text
//!
//! A pattern over the text reads `Blocked by #168` on one line, and the list
//! marker between the heading and the number stops it. So the body is cut into
//! blocks first: headings, paragraphs and list items, with code fences skipped.
//! A block names a blocker in two ways:
//!
//! * It stands in the section of a `Blocked by` or `Depends on` heading, up to
//!   the next heading of the same level or higher.
//! * Its own text starts with one of those labels, as in `**Blocked by:** #12`.
//!
//! # Only the numbers at the start of a block count
//!
//! `#3, #4 and #5` and `#5 (test-taking UI)` are blockers. A block that starts
//! with a word is prose about other work, so `It can run beside #169.` under the
//! heading names no blocker. A paragraph that wraps is one block, so a line that
//! happens to start with a number is still inside that prose. A number struck
//! through, as in `~~#21~~`, counts for nothing, because an author strikes a
//! blocker through to take it back.
//!
//! This reader acts on what it reads: a blocker it names can refuse a plan. So a
//! phrase in the middle of a sentence is not read, because a line of a tracker
//! such as `#12 — Click to open *blocked by #11*` says what blocks another issue.
//!
//! The gather script of the `plan-parallel-work` skill reads the same blocks,
//! and its table of forms stands beside the table of this module.

use crate::chain::IssueNumber;

/// The labels that name work which comes before the issue. A heading carries
/// one to open a section, and a block carries one at its start. They are ASCII,
/// so a comparison that ignores ASCII case is the whole comparison.
const LABELS: &[&str] = &["blocked by", "depends on"];

/// The word a separator between two numbers can be: `#4 and #5`.
const AND: &str = "and";

/// The most spaces a heading, a fence, or a block quote is indented by. One more
/// makes the line code.
const MAX_INDENT: usize = 3;

/// The fewest marks that open a code fence.
const FENCE_MARKS: usize = 3;

/// The most tildes that open a span struck through. One more opens a code fence
/// at the start of a line, and it strikes nothing inside a block.
const MAX_STRIKE_MARKS: usize = 2;

/// The deepest level of a heading.
const MAX_HEADING_LEVEL: usize = 6;

/// The most digits the number of an ordered list item has.
const MAX_ITEM_DIGITS: usize = 9;

/// One block of a body.
enum Block {
    /// A heading, its level, and its text with the marks taken off.
    Heading { level: usize, text: String },
    /// A paragraph or a list item, with the lines that continue it.
    Text(String),
}

/// Whether a block that is still taking lines is a paragraph or a list item.
///
/// Only a paragraph can become a setext heading.
#[derive(PartialEq, Eq)]
enum Kind {
    Paragraph,
    Item,
}

/// A block that is still taking lines.
struct Open {
    kind: Kind,
    lines: Vec<String>,
}

/// The numbers `body` names as work that comes before its issue, in the order
/// the body writes them, each one once.
///
/// A number of another repository is read past and names nothing, and so is a
/// span struck through and a number GitHub cannot give an issue: zero, or one
/// too large for a `u64`.
#[must_use]
pub fn read(body: &str) -> Vec<IssueNumber> {
    let mut numbers: Vec<IssueNumber> = Vec::new();
    // The level of the heading whose section the walk stands in.
    let mut section: Option<usize> = None;
    for block in blocks_of(body) {
        match block {
            Block::Heading { level, text } => {
                if section.is_some_and(|open| level <= open) {
                    section = None;
                }
                if section.is_none() && after_label(undecorated(&text)).is_some() {
                    section = Some(level);
                }
            }
            Block::Text(text) => {
                let (labelled, named) = head_of(&text);
                if section.is_none() && !labelled {
                    continue;
                }
                for number in named {
                    if !numbers.contains(&number) {
                        numbers.push(number);
                    }
                }
            }
        }
    }
    numbers
}

/// The headings, paragraphs and list items of `body`, in order.
///
/// A list item takes the lines that continue it, and a paragraph takes the lines
/// that wrap it. A code fence and an indented code block give no block, and a
/// block quote gives the blocks inside it.
fn blocks_of(body: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Open> = None;
    let mut fence: Option<(char, usize)> = None;
    for written in body.split('\n') {
        let mut line = written.strip_suffix('\r').unwrap_or(written);
        while let Some(inner) = quoted(line) {
            line = inner;
        }

        if let Some((mark, length)) = fence {
            if closes_fence(line, mark, length) {
                fence = None;
            }
            continue;
        }
        if let Some(opened) = fence_of(line) {
            close(&mut open, &mut blocks);
            fence = Some(opened);
            continue;
        }
        if line.trim().is_empty() {
            close(&mut open, &mut blocks);
            continue;
        }
        if let Some((level, text)) = atx_heading(line) {
            close(&mut open, &mut blocks);
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            continue;
        }
        if let Some(level) = setext_underline(line) {
            match open.take() {
                Some(Open {
                    kind: Kind::Paragraph,
                    lines,
                }) => blocks.push(Block::Heading {
                    level,
                    text: lines.join(" "),
                }),
                other => {
                    open = other;
                    close(&mut open, &mut blocks);
                }
            }
            continue;
        }
        if open.is_none() && is_indented_code(line) {
            continue;
        }
        if let Some(text) = list_item(line) {
            close(&mut open, &mut blocks);
            open = Some(Open {
                kind: Kind::Item,
                lines: vec![text.trim().to_string()],
            });
            continue;
        }
        match &mut open {
            Some(block) => block.lines.push(line.trim().to_string()),
            None => {
                open = Some(Open {
                    kind: Kind::Paragraph,
                    lines: vec![line.trim().to_string()],
                });
            }
        }
    }
    close(&mut open, &mut blocks);
    blocks
}

/// Put the block that is still taking lines, if there is one, at the end of
/// `blocks`.
fn close(open: &mut Option<Open>, blocks: &mut Vec<Block>) {
    if let Some(block) = open.take() {
        blocks.push(Block::Text(block.lines.join("\n")));
    }
}

/// `line` with its indentation taken off, or `None` when it is indented by more
/// than [`MAX_INDENT`] spaces.
fn unindented(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(' ');
    (line.len() - rest.len() <= MAX_INDENT).then_some(rest)
}

/// The text inside one level of block quote, or `None` when `line` is not quoted.
fn quoted(line: &str) -> Option<&str> {
    let rest = unindented(line)?.strip_prefix('>')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// The mark and the length of the code fence `line` opens, or `None` when it
/// opens none.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let rest = unindented(line)?;
    let mark = rest.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = rest.chars().take_while(|c| *c == mark).count();
    (length >= FENCE_MARKS).then_some((mark, length))
}

/// Whether `line` closes a fence of `length` marks of `mark`.
fn closes_fence(line: &str, mark: char, length: usize) -> bool {
    unindented(line).is_some_and(|rest| {
        let after = rest.trim_start_matches(mark);
        rest.chars().count() - after.chars().count() >= length && after.trim().is_empty()
    })
}

/// The level and the text of the ATX heading `line` is, or `None` when it is
/// no such heading.
///
/// `#168` is no heading, because a heading puts a space after its marks.
fn atx_heading(line: &str) -> Option<(usize, &str)> {
    let rest = unindented(line)?;
    let after = rest.trim_start_matches('#');
    let level = rest.len() - after.len();
    if !(1..=MAX_HEADING_LEVEL).contains(&level) {
        return None;
    }
    if !(after.is_empty() || after.starts_with([' ', '\t'])) {
        return None;
    }
    let text = after.trim();
    let unclosed = text.trim_end_matches('#');
    if unclosed.is_empty() || unclosed.ends_with([' ', '\t']) {
        return Some((level, unclosed.trim_end()));
    }
    Some((level, text))
}

/// The level of the setext heading `line` underlines, or `None` when it is no
/// such underline.
fn setext_underline(line: &str) -> Option<usize> {
    let rest = unindented(line)?.trim_end_matches([' ', '\t']);
    let mark = rest.chars().next().filter(|c| matches!(c, '=' | '-'))?;
    rest.chars()
        .all(|c| c == mark)
        .then_some(if mark == '=' { 1 } else { 2 })
}

/// Whether `line` is indented far enough to be code.
fn is_indented_code(line: &str) -> bool {
    line.starts_with('\t') || unindented(line).is_none()
}

/// The text of the list item `line` opens, or `None` when it opens none.
///
/// The marker can stand at any depth, so a nested item opens a block of its
/// own and does not continue the item above it.
fn list_item(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches([' ', '\t']);
    let after = if let Some(after) = rest.strip_prefix(['-', '*', '+']) {
        after
    } else {
        let digits = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        if !(1..=MAX_ITEM_DIGITS).contains(&(rest.len() - digits.len())) {
            return None;
        }
        digits.strip_prefix(['.', ')'])?
    };
    if after.is_empty() {
        return Some(after);
    }
    after.strip_prefix([' ', '\t'])
}

/// Whether `c` is a character of a word, as the boundary after a label and
/// after a number reads it.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `c` is something a writer puts around a label and a number: space,
/// an emphasis mark, or a symbol such as `⛔` or `🚧`.
///
/// The symbols are named as ranges, and the gather script of the skill names the
/// same ranges, so the two readers strip the same characters: U+2190 to U+2BFF
/// (arrows, shapes, dingbats, the older emoji), U+1F000 to U+1FAFF (the newer
/// emoji), the zero width joiner, and the emoji selector.
///
/// A tilde is not decoration. It opens a span struck through, and
/// [`after_strike`] reads past that span, so a struck label or number names
/// nothing.
fn is_decoration(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '*' | '_'
                | '\u{200D}'
                | '\u{FE0F}'
                | '\u{2190}'..='\u{2BFF}'
                | '\u{1F000}'..='\u{1FAFF}'
        )
}

/// `text` with the decoration in front of it taken off.
fn undecorated(text: &str) -> &str {
    text.trim_start_matches(is_decoration)
}

/// The text after the label `text` starts with, or `None` when it starts with
/// none.
fn after_label(text: &str) -> Option<&str> {
    LABELS.iter().find_map(|label| {
        let head = text.get(..label.len())?;
        let rest = text.get(label.len()..)?;
        (head.eq_ignore_ascii_case(label) && !rest.chars().next().is_some_and(is_word))
            .then_some(rest)
    })
}

/// Whether the text of a block starts with a label, and the numbers at its
/// start.
fn head_of(text: &str) -> (bool, Vec<IssueNumber>) {
    let mut rest = undecorated(without_task_box(text));
    let labelled = match after_label(rest) {
        Some(after) => {
            let after = undecorated(after);
            rest = after.strip_prefix(':').unwrap_or(after);
            true
        }
        None => false,
    };

    let mut numbers: Vec<IssueNumber> = Vec::new();
    loop {
        rest = undecorated(rest);
        if let Some((number, after)) = local_reference(rest) {
            numbers.extend(number);
            rest = after;
        } else if let Some(after) = read_past(rest) {
            rest = after;
        } else {
            break;
        }
        rest = rest.trim_start_matches([' ', '\t']);
        if rest.starts_with('(') {
            if let Some(after) = after_group(rest) {
                rest = after;
            }
        }
        rest = undecorated(rest);
        if let Some(after) = separator(rest) {
            rest = after;
        } else if local_reference(rest).is_none() && read_past(rest).is_none() {
            break;
        }
    }
    (labelled, numbers)
}

/// `text` with the box of a task list item taken off its front.
fn without_task_box(text: &str) -> &str {
    ["[ ]", "[x]", "[X]"]
        .iter()
        .find_map(|task_box| {
            let after = text.strip_prefix(task_box)?;
            let spaced = after.trim_start_matches([' ', '\t']);
            (spaced.len() < after.len()).then_some(spaced)
        })
        .unwrap_or(text)
}

/// The number of this repository `text` starts with, and the text after it, or
/// `None` when it starts with no such number.
///
/// The number is `None` when GitHub cannot give an issue that number. The text
/// after it still comes back, so the caller reads past it.
fn local_reference(text: &str) -> Option<(Option<IssueNumber>, &str)> {
    let after_mark = text.strip_prefix('#')?;
    let end = after_mark
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_mark.len());
    if end == 0 {
        return None;
    }
    let (digits, after) = after_mark.split_at(end);
    if after.starts_with('#') || after.chars().next().is_some_and(is_word) {
        return None;
    }
    Some((digits.parse().ok().and_then(IssueNumber::new), after))
}

/// The text after the number of another repository `text` starts with, as in
/// `timmattison/muxiavelli#294`, or `None` when it starts with none.
fn other_reference(text: &str) -> Option<&str> {
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
    let after_owner = text.trim_start_matches(is_name);
    if after_owner.len() == text.len() {
        return None;
    }
    let name = after_owner.strip_prefix('/')?;
    let after_name = name.trim_start_matches(is_name);
    if after_name.len() == name.len() {
        return None;
    }
    local_reference(after_name).map(|(_, after)| after)
}

/// The text after what `text` starts with that names nothing and that a list
/// continues past, or `None` when it starts with no such thing: a number of
/// another repository, or a span struck through.
fn read_past(text: &str) -> Option<&str> {
    other_reference(text).or_else(|| after_strike(text))
}

/// The text after the span struck through that `text` opens with, or `None`
/// when it opens none.
///
/// One or two tildes open the span, as GitHub renders both `~#21~` and
/// `~~#21~~`. The next run of the same number of tildes closes it. A span that
/// nothing closes is no strike, and the tildes stay in front of the text.
fn after_strike(text: &str) -> Option<&str> {
    let inside = text.trim_start_matches('~');
    let marks = text.len() - inside.len();
    if !(1..=MAX_STRIKE_MARKS).contains(&marks) {
        return None;
    }
    let mut rest = inside;
    loop {
        let run = rest.get(rest.find('~')?..)?;
        let after = run.trim_start_matches('~');
        if run.len() - after.len() == marks {
            return Some(after);
        }
        rest = after;
    }
}

/// The text after the parenthesis that closes the one `text` opens with, or
/// `None` when nothing closes it.
fn after_group(text: &str) -> Option<&str> {
    let mut depth: usize = 0;
    for (at, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return text.get(at + 1..);
                }
            }
            _ => {}
        }
    }
    None
}

/// The text after the separator `text` starts with, or `None` when it starts
/// with none: a comma, an ampersand, a slash, or the word `and`.
fn separator(text: &str) -> Option<&str> {
    if let Some(after) = text.strip_prefix([',', '&', '/']) {
        return Some(after);
    }
    let head = text.get(..AND.len())?;
    let after = text.get(AND.len()..)?;
    (head.eq_ignore_ascii_case(AND) && !after.chars().next().is_some_and(is_word)).then_some(after)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One body, and the numbers it names as blockers.
    struct Case {
        name: &'static str,
        body: &'static str,
        numbers: &'static [u64],
    }

    /// One case for each syntactic form a blocker arrives in, and one for each
    /// form that names an issue and is not a blocker.
    ///
    /// `.claude/skills/plan-parallel-work/scripts/dependency-sections.test.ts`
    /// in timmattison/dotfiles holds the same forms for the gather script of the
    /// skill that writes the plans. A form added here belongs there too.
    const CASES: &[Case] = &[
        // The heading, in the spellings it arrives in.
        Case { name: "a list item under a Blocked by heading", body: "## Blocked by\n\n- #168\n", numbers: &[168] },
        Case { name: "a heading in title case", body: "## Blocked By\n\n- #7\n", numbers: &[7] },
        Case { name: "a heading with closing marks at a deeper level", body: "### Blocked by ###\n\n- #7\n", numbers: &[7] },
        Case {
            name: "a heading with a symbol in front and words after it",
            body: "## \u{26d4} Blocked by (do not start until these land)\n\n- #7\n",
            numbers: &[7],
        },
        Case { name: "a heading indented by three spaces", body: "   ## Blocked by\n\n- #7\n", numbers: &[7] },
        Case { name: "a setext heading", body: "Blocked by\n----------\n\n- #7\n", numbers: &[7] },
        Case { name: "a body with Windows line endings", body: "## Blocked by\r\n\r\n- #168\r\n", numbers: &[168] },
        Case {
            name: "a Depends on heading",
            body: "## Depends on\n\n**#17 (R2 upload).** There is nothing in the bucket until #17 runs.\n",
            numbers: &[17],
        },
        // The block under the heading, in the shapes it arrives in.
        Case { name: "every list marker", body: "## Blocked by\n\n* #1\n+ #2\n1. #3\n2) #4\n", numbers: &[1, 2, 3, 4] },
        Case { name: "a task list item, open and done", body: "## Blocked by\n\n- [ ] #5\n- [x] #6\n", numbers: &[5, 6] },
        Case {
            name: "a number with an annotation after it",
            body: "## Blocked by\n\n- #5 (test-taking UI)\n- #296 \u{2014} Create and switch workspaces\n",
            numbers: &[5, 296],
        },
        Case {
            name: "a list item that repeats the label",
            body: "## Blocked by\n\n- Blocked by #26 (slice 1 scaffolding)\n",
            numbers: &[26],
        },
        Case { name: "several numbers in one item", body: "## Blocked by\n\n- #3, #4 and #5\n", numbers: &[3, 4, 5] },
        Case { name: "a bare number as a paragraph", body: "## Blocked by\n\n#168\n", numbers: &[168] },
        Case {
            name: "a nested list item indented by four spaces",
            body: "## Blocked by\n\n- The solver\n    - #168\n",
            numbers: &[168],
        },
        Case {
            name: "a subheading inside the section",
            body: "## Blocked by\n\n### Before the solver\n\n- #11\n",
            numbers: &[11],
        },
        Case { name: "one number named twice", body: "## Blocked by\n\n- #9\n- #9 (again)\n", numbers: &[9] },
        // The forms that name an issue and are not a blocker.
        Case {
            name: "prose under the heading that names another issue",
            body: "## Blocked by\n\n- #168\n\nIt can run beside #169. Both touch the widget.\n",
            numbers: &[168],
        },
        Case {
            name: "a wrapped paragraph whose second line starts with a number",
            body: "## Blocked by\n\n- #168\n\nIt can run beside\n#169 when the two do not overlap.\n",
            numbers: &[168],
        },
        Case {
            name: "a list item whose second line starts with a number",
            body: "## Blocked by\n\n- The slope solver, which lands in\n  #168 first\n",
            numbers: &[],
        },
        Case { name: "no blocker at all", body: "## Blocked by\n\nNone - can start immediately.\n", numbers: &[] },
        Case {
            name: "a remark in parentheses",
            body: "## Blocked by\n\n(The surviving issues #38/#39 still need triage.)\n",
            numbers: &[],
        },
        Case {
            name: "a number in a code span",
            body: "## Blocked by\n\n- `#9` was the old number of this work\n",
            numbers: &[],
        },
        Case { name: "a heading inside a code fence", body: "```md\n## Blocked by\n\n- #9\n```\n", numbers: &[] },
        Case {
            name: "a list item inside a code fence under the heading",
            body: "## Blocked by\n\n~~~\n- #9\n~~~\n\n- #11\n",
            numbers: &[11],
        },
        Case { name: "a Blocks heading, which is the other direction", body: "## Blocks\n\n- #12\n", numbers: &[] },
        Case {
            name: "a heading of the same level ends the section",
            body: "## Blocked by\n\n- #11\n\n## Acceptance criteria\n\n- #12 still passes\n",
            numbers: &[11],
        },
        Case {
            name: "a Blocks heading at the level of the section ends it",
            body: "### Blocked by\n\n- #11\n\n### Blocks\n\n- #12\n",
            numbers: &[11],
        },
        Case {
            name: "a number of another repository",
            body: "## Blocked by\n\n- timmattison/muxiavelli#294\n- #5\n",
            numbers: &[5],
        },
        Case {
            name: "a blocker struck through",
            body: "## Blocked by\n\n- ~~#21~~ (no longer needed)\n",
            numbers: &[],
        },
        Case { name: "a blocker struck through with one tilde", body: "## Blocked by\n\n- ~#21~\n", numbers: &[] },
        Case { name: "a struck blocker and a live one", body: "## Blocked by\n\n- ~~#21~~ #22\n", numbers: &[22] },
        Case {
            name: "a struck blocker and a live one after a comma",
            body: "## Blocked by\n\n- ~~#21~~, #22\n",
            numbers: &[22],
        },
        Case { name: "a strike that nothing closes", body: "## Blocked by\n\n- ~~#21 #22\n", numbers: &[] },
        Case { name: "a label struck through", body: "~~**Blocked by:** #12~~\n", numbers: &[] },
        Case { name: "a heading struck through", body: "## ~~Blocked by~~\n\n- #7\n", numbers: &[] },
        Case { name: "a number that is zero", body: "## Blocked by\n\n- #0\n", numbers: &[] },
        Case {
            name: "a number too large for any issue",
            body: "## Blocked by\n\n- #99999999999999999999999\n",
            numbers: &[],
        },
        // A label at the start of a block, with no heading.
        Case { name: "a bold label with a colon", body: "**Blocked by:** #12\n", numbers: &[12] },
        Case {
            name: "a bold label in a list item with two numbers",
            body: "- **Blocked by:** #11 (Phase 1 \u{2014} Scaffold) and #12 (Phase 2 \u{2014} routes).\n",
            numbers: &[11, 12],
        },
        Case {
            name: "a label in a block quote behind a symbol",
            body: "> **\u{1f6a7} Blocked by #12.** Land it before this one.\n",
            numbers: &[12],
        },
        Case { name: "a label that names no issue", body: "**Blocked by:** none.\n", numbers: &[] },
        Case {
            name: "a label at the start of a paragraph",
            body: "Blocked by #9. It edits crates/tsm/src/serve.rs.",
            numbers: &[9],
        },
        Case {
            name: "a label and a section, in the order the body writes them",
            body: "Depends on #3 for the schema.\n\n## Blocked by\n\n- #4\n",
            numbers: &[3, 4],
        },
        // The forms the gather script reads and this reader does not, because
        // neither of them says which issue waits.
        Case { name: "a parent heading", body: "## Parent\n\n#166\n", numbers: &[] },
        Case { name: "a parent label", body: "- **Parent:** #61.\n", numbers: &[] },
        Case {
            name: "a phrase in the middle of a sentence",
            body: "This slice is blocked by #9 until it lands.\n",
            numbers: &[],
        },
        Case {
            name: "a line of a tracker that says what blocks another issue",
            body: "- [ ] #12 \u{2014} Click to open *blocked by #11*\n",
            numbers: &[],
        },
    ];

    #[test]
    fn every_form_of_a_blocker_reads_as_its_case_says() {
        let wrong: Vec<String> = CASES
            .iter()
            .filter_map(|case| {
                let read: Vec<u64> = read(case.body).iter().map(|number| number.get()).collect();
                (read != case.numbers).then(|| {
                    format!(
                        "{}: read {read:?} and wanted {:?} out of {:?}",
                        case.name, case.numbers, case.body
                    )
                })
            })
            .collect();
        assert!(
            wrong.is_empty(),
            "{} of {} cases read wrong:\n{}",
            wrong.len(),
            CASES.len(),
            wrong.join("\n")
        );
    }
}
