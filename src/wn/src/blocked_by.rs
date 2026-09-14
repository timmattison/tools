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
//! blocks first: headings, paragraphs and list items, with code fences and HTML
//! comments skipped. GitHub does not show a comment, and an issue template
//! writes its examples in one. A block names a blocker in two ways:
//!
//! * It stands in the section of a `Blocked by` or `Depends on` heading, up to
//!   the next heading of the same level or higher.
//! * Its own text starts with one of those labels, as in `**Blocked by:** #12`.
//!
//! A heading is a block too, so the same two rules read its own text:
//! `## Blocked by #41` opens a section and names #41. A heading that ends a
//! section stands outside it.
//!
//! # Only the numbers at the start of a block count
//!
//! `#3, #4 and #5` and `#5 (test-taking UI)` are blockers, and so is the URL
//! of an issue or a pull request of the repository, as in
//! `https://github.com/owner/name/issues/5`. A block that starts with a word is
//! prose about other work, so `It can run beside #169.` under the heading names
//! no blocker. A paragraph that wraps is one block, so a line that
//! happens to start with a number is still inside that prose. A number struck
//! through, as in `~~#21~~`, counts for nothing, because an author strikes a
//! blocker through to take it back. Tildes strike through only where GitHub
//! strikes through ([`after_strike`](crate::strike::after_strike) gives the
//! rule), so the item `~~#21 ~~ #22`
//! starts with tildes and names no blocker.
//!
//! This reader acts on what it reads: a blocker it names can refuse a plan. So a
//! phrase in the middle of a sentence is not read, because a line of a tracker
//! such as `#12 — Click to open *blocked by #11*` says what blocks another issue.
//!
//! The gather script of the `plan-parallel-work` skill reads the same blocks,
//! and its table of forms stands beside the table of this module.

use crate::chain::IssueNumber;
use crate::github::Repo;

/// The labels that name work which comes before the issue. A heading carries
/// one to open a section, and a block carries one at its start. They are ASCII,
/// so a comparison that ignores ASCII case is the whole comparison.
const LABELS: &[&str] = &["blocked by", "depends on"];

/// The word a separator between two numbers can be: `#4 and #5`.
const AND: &str = "and";

/// The most spaces a heading, a fence, or a block quote is indented by. One more
/// makes the line code.
const MAX_INDENT: usize = 3;

/// The columns of indent, past the content of the list item a line stands in,
/// that make the line indented code.
const CODE_INDENT: usize = MAX_INDENT + 1;

/// A tab moves a line to the next column that is a multiple of this width.
const TAB_WIDTH: usize = 4;

/// The fewest marks that open a code fence.
const FENCE_MARKS: usize = 3;

/// The text a line starts with to open an HTML comment.
const COMMENT_OPEN: &str = "<!--";

/// The text that closes an HTML comment.
const COMMENT_CLOSE: &str = "-->";

/// The deepest level of a heading.
const MAX_HEADING_LEVEL: usize = 6;

/// The most digits the number of an ordered list item has.
const MAX_ITEM_DIGITS: usize = 9;

/// The schemes the URL of an issue starts with.
const SCHEMES: &[&str] = &["https://", "http://"];

/// The parts of a path that name an issue and a pull request, as in
/// `issues/51` and `pull/52`.
const ISSUE_PATHS: &[&str] = &["issues", "pull"];

/// The mark that opens an autolink, as in `<https://github.com/o/n/issues/5>`.
const AUTOLINK_OPEN: char = '<';

/// The mark that closes an autolink.
const AUTOLINK_CLOSE: char = '>';

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

/// A column of a line, counted from zero.
///
/// A column is not a byte offset. A tab moves to the next column that is a
/// multiple of [`TAB_WIDTH`], so one tab can fill more than one column.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Column(usize);

impl Column {
    /// The column a line starts at.
    const START: Self = Self(0);

    /// The column after `c`, when `c` stands at this column.
    fn after(self, c: char) -> Self {
        if c == '\t' {
            Self(self.0 + TAB_WIDTH - self.0 % TAB_WIDTH)
        } else {
            Self(self.0 + 1)
        }
    }

    /// The column after `text`, when `text` starts at this column.
    fn past(self, text: &str) -> Self {
        text.chars().fold(self, Self::after)
    }

    /// The number of columns from `start` to this column, or zero when `start`
    /// stands after this column.
    fn beyond(self, start: Self) -> usize {
        self.0.saturating_sub(start.0)
    }
}

/// A list item that a line opens.
struct Item<'a> {
    /// The text after the marker and the space after it.
    text: &'a str,
    /// The column the content of the item starts at. A later line indented to
    /// this column stands inside the item.
    content: Column,
}

/// The numbers `body` names as work that comes before its issue, in the order
/// the body writes them, each one once.
///
/// `repo` is the repository of the issue. A block names a blocker as a number,
/// as in `#51`, or as the URL of an issue or a pull request of `repo`, as in
/// `https://github.com/owner/name/issues/51`.
///
/// A number or a URL of another repository is read past and names nothing, and
/// so is a span struck through and a number GitHub cannot give an issue: zero,
/// or one too large for a `u64`.
///
/// A heading ends or opens a section before the walk reads its own text. So a
/// heading that ends a section names nothing unless it starts with a label.
#[must_use]
pub fn read(body: &str, repo: &Repo) -> Vec<IssueNumber> {
    let mut numbers: Vec<IssueNumber> = Vec::new();
    // The level of the heading whose section the walk stands in.
    let mut section: Option<usize> = None;
    for block in blocks_of(body) {
        let (level, text) = match block {
            Block::Heading { level, text } => (Some(level), text),
            Block::Text(text) => (None, text),
        };
        let (labelled, named) = head_of(&text, repo);
        if let Some(level) = level {
            if section.is_some_and(|open| level <= open) {
                section = None;
            }
            if section.is_none() && labelled {
                section = Some(level);
            }
        }
        if section.is_none() && !labelled {
            continue;
        }
        for number in named {
            if !numbers.contains(&number) {
                numbers.push(number);
            }
        }
    }
    numbers
}

/// The headings, paragraphs and list items of `body`, in order.
///
/// A list item takes the lines that continue it, and a paragraph takes the lines
/// that wrap it. A code fence, an indented code block, and an HTML comment give
/// no block, and a block quote gives the blocks inside it.
///
/// A blank line ends a paragraph and a list item, but not the list. The content
/// column of an item is the column of its text after the marker. A later line
/// indented to that column stands inside the item, so it is a nested item, a
/// paragraph, a fence or a comment, as at the start of a line. It is indented
/// code only at [`CODE_INDENT`] columns past that column. A line indented less
/// than the column, or a line at another quote depth, ends the item. A line that
/// continues the open paragraph or item changes no list.
///
/// An HTML comment starts at a line that opens with [`COMMENT_OPEN`] and ends at
/// the first line that holds [`COMMENT_CLOSE`], which can be the line that opens
/// it. A comment that nothing closes runs to the end of the body. A comment
/// inside a code fence is text of the fence, and a comment in the middle of a
/// line is text of its block.
fn blocks_of(body: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut open: Option<Open> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut comment = false;
    // The content columns of the open list items, outermost first, so the
    // columns rise. `items_depth` is the quote depth of their list.
    let mut items: Vec<Column> = Vec::new();
    let mut items_depth: usize = 0;
    for written in body.split('\n') {
        let mut line = written.strip_suffix('\r').unwrap_or(written);
        let mut depth: usize = 0;
        while let Some(inner) = quoted(line) {
            line = inner;
            depth += 1;
        }

        // `rest` is the line as it stands inside the innermost item it is
        // indented into, with its indent written in spaces. `inside` is the
        // number of open items the line is indented into.
        let words = line.trim_start_matches([' ', '\t']);
        let indent = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .fold(Column::START, Column::after);
        let inside = if depth == items_depth {
            items.partition_point(|column| *column <= indent)
        } else {
            0
        };
        let top = items
            .get(..inside)
            .and_then(<[Column]>::last)
            .copied()
            .unwrap_or(Column::START);
        let rest = format!("{}{words}", " ".repeat(indent.beyond(top)));

        if let Some((mark, length)) = fence {
            if closes_fence(&rest, mark, length) {
                fence = None;
            }
            continue;
        }
        if comment {
            comment = !line.contains(COMMENT_CLOSE);
            continue;
        }
        if line.trim().is_empty() {
            close(&mut open, &mut blocks);
            continue;
        }
        let opened = fence_of(&rest);
        let comment_opens = opens_comment(&rest);
        let heading = atx_heading(&rest);
        let underline = setext_underline(&rest);
        let item = list_item(&rest, top);
        // A line that starts a new block ends every item it is indented less
        // than. A line that continues the open block changes no list.
        let continues = open.as_ref().is_some_and(|block| {
            opened.is_none()
                && !comment_opens
                && heading.is_none()
                && item.is_none()
                && (underline.is_none() || block.kind == Kind::Paragraph)
        });
        if !continues {
            items.truncate(inside);
            items_depth = depth;
        }

        if let Some(opened) = opened {
            close(&mut open, &mut blocks);
            fence = Some(opened);
            continue;
        }
        if comment_opens {
            close(&mut open, &mut blocks);
            comment = !line.contains(COMMENT_CLOSE);
            continue;
        }
        if let Some((level, text)) = heading {
            close(&mut open, &mut blocks);
            blocks.push(Block::Heading {
                level,
                text: text.to_string(),
            });
            continue;
        }
        if let Some(level) = underline {
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
        if open.is_none() && indent.beyond(top) >= CODE_INDENT {
            continue;
        }
        if let Some(Item { text, content }) = item {
            close(&mut open, &mut blocks);
            items.push(content);
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

/// Whether `line` opens an HTML comment.
///
/// The comment can stop a paragraph or a list item, as CommonMark lets an HTML
/// block of its second type do. A line indented by more than [`MAX_INDENT`]
/// spaces opens no comment, because it is code or it continues a block.
fn opens_comment(line: &str) -> bool {
    unindented(line).is_some_and(|rest| rest.starts_with(COMMENT_OPEN))
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

/// The list item `line` opens, or `None` when it opens none. `line` starts at
/// column `from`.
///
/// The marker can stand at any depth, so a nested item opens a block of its
/// own and does not continue the item above it.
///
/// The content of the item starts after the spaces that follow the marker. An
/// item with no text puts its content one column past the marker.
fn list_item(line: &str, from: Column) -> Option<Item<'_>> {
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
    let text = after.trim_start_matches([' ', '\t']);
    let marker = from.past(line.get(..line.len() - after.len())?);
    if text.is_empty() {
        return Some(Item {
            text,
            content: marker.after(' '),
        });
    }
    let space = after.get(..after.len() - text.len())?;
    (!space.is_empty()).then(|| Item {
        text,
        content: marker.past(space),
    })
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
/// A tilde is not decoration. It can open a span struck through, and
/// [`after_strike`](crate::strike::after_strike) reads past that span, so a
/// struck label or number names
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
/// start. `repo` is the repository of the issue.
fn head_of(text: &str, repo: &Repo) -> (bool, Vec<IssueNumber>) {
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
        if let Some((number, after)) = reference(rest, repo) {
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
        } else if reference(rest, repo).is_none() && read_past(rest).is_none() {
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

/// The number of this repository `text` starts with, as `#51` or as the URL of
/// an issue of `repo`, and the text after it, or `None` when it starts with
/// neither.
///
/// The number is `None` for a URL of another repository, and when GitHub
/// cannot give an issue that number. The text after it still comes back, so
/// the caller reads past it.
fn reference<'a>(text: &'a str, repo: &Repo) -> Option<(Option<IssueNumber>, &'a str)> {
    local_reference(text).or_else(|| url_reference(text, repo))
}

/// The number `text` starts with, written as `#51`, and the text after it, or
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

/// The number of the URL of an issue or a pull request that `text` starts
/// with, and the text after the URL, or `None` when it starts with no such URL.
///
/// The URL is a scheme of [`SCHEMES`], a host, the owner and the name, and then
/// a path of [`ISSUE_PATHS`] and the number, as in
/// `https://github.com/owner/name/issues/51`. A slash after the number and a
/// fragment such as `#issuecomment-7` are part of the URL. An autolink puts
/// [`AUTOLINK_OPEN`] and [`AUTOLINK_CLOSE`] around it. Any host counts, because
/// `gh` serves an enterprise host too and [`Repo`] holds no host.
///
/// The number is `None` when the owner and the name are not those of `repo`,
/// and when GitHub cannot give an issue that number. The comparison ignores
/// ASCII case, as GitHub does. A URL whose path names something other than an
/// issue or a pull request, such as a file, gives `None`, so the list ends at
/// it.
fn url_reference<'a>(text: &'a str, repo: &Repo) -> Option<(Option<IssueNumber>, &'a str)> {
    let autolink = text.strip_prefix(AUTOLINK_OPEN);
    let url = autolink.unwrap_or(text);
    let after_scheme = SCHEMES.iter().find_map(|scheme| url.strip_prefix(scheme))?;
    let (_, after_host) = segment(after_scheme, is_host)?;
    let (owner, after_owner) = segment(after_host, is_name)?;
    let (name, after_name) = segment(after_owner, is_name)?;
    let after_path = ISSUE_PATHS
        .iter()
        .find_map(|path| after_name.strip_prefix(path)?.strip_prefix('/'))?;
    let end = after_path
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_path.len());
    if end == 0 {
        return None;
    }
    let (digits, after_digits) = after_path.split_at(end);
    let rest = after_digits.strip_prefix('/').unwrap_or(after_digits);
    let rest = rest
        .strip_prefix('#')
        .map_or(rest, |fragment| fragment.trim_start_matches(is_fragment));
    let after = match autolink {
        Some(_) => rest.strip_prefix(AUTOLINK_CLOSE)?,
        None => rest,
    };
    if after
        .chars()
        .next()
        .is_some_and(|c| is_word(c) || matches!(c, '/' | '#'))
    {
        return None;
    }
    let same = owner.eq_ignore_ascii_case(repo.owner()) && name.eq_ignore_ascii_case(repo.name());
    let number = digits.parse().ok().and_then(IssueNumber::new);
    Some((number.filter(|_| same), after))
}

/// The run of characters at the start of `text` that `is_part` accepts, and
/// the text after the slash that ends the run, or `None` when the run is empty
/// or no slash ends it.
fn segment(text: &str, is_part: fn(char) -> bool) -> Option<(&str, &str)> {
    let after = text.trim_start_matches(is_part);
    let part = text.get(..text.len() - after.len())?;
    if part.is_empty() {
        return None;
    }
    Some((part, after.strip_prefix('/')?))
}

/// Whether `c` is a character of the host of a URL, with its port.
fn is_host(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')
}

/// Whether `c` is a character of the owner or the name of a repository.
fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')
}

/// Whether `c` is a character of the fragment GitHub puts after the URL of an
/// issue, as in `#issuecomment-7` or `#discussion_r12`.
fn is_fragment(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
}

/// The text after the number of another repository `text` starts with, as in
/// `timmattison/muxiavelli#294`, or `None` when it starts with none.
fn other_reference(text: &str) -> Option<&str> {
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
    other_reference(text).or_else(|| crate::strike::after_strike(text))
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
///
/// A comma and the word `and` after it are one separator, as in the serial
/// comma of `#3, #4, and #5`. Decoration can stand between the two. An
/// ampersand and a slash take no word after them.
fn separator(text: &str) -> Option<&str> {
    if let Some(after) = text.strip_prefix(',') {
        return Some(after_and(undecorated(after)).unwrap_or(after));
    }
    if let Some(after) = text.strip_prefix(['&', '/']) {
        return Some(after);
    }
    after_and(text)
}

/// The text after the word `and` that `text` starts with, or `None` when it
/// starts with no such word.
fn after_and(text: &str) -> Option<&str> {
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
        Case { name: "a heading that names its blocker on its own line", body: "## Blocked by #41\n\nLand it first.\n", numbers: &[41] },
        Case { name: "a heading with a colon and two numbers", body: "## Blocked by: #41 and #42\n", numbers: &[41, 42] },
        Case { name: "a heading with a number and an item under it", body: "### Blocked by #41\n\n- #42\n", numbers: &[41, 42] },
        Case { name: "a setext heading with a number", body: "Blocked by #41\n--------------\n\n- #42\n", numbers: &[41, 42] },
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
        Case { name: "several numbers with a serial comma", body: "## Blocked by\n\n- #3, #4, and #5\n", numbers: &[3, 4, 5] },
        Case { name: "two numbers with a comma and the word and", body: "## Blocked by\n\n- #3, and #4\n", numbers: &[3, 4] },
        Case { name: "a bold label with a serial comma", body: "**Blocked by:** #11, #12, and #13.\n", numbers: &[11, 12, 13] },
        Case { name: "a comma and the word and before prose", body: "## Blocked by\n\n- #3, and it lands first\n", numbers: &[3] },
        Case { name: "a bare number as a paragraph", body: "## Blocked by\n\n#168\n", numbers: &[168] },
        Case {
            name: "the URL of an issue of the repository",
            body: "## Blocked by\n\n- https://github.com/timmattison/example/issues/51\n",
            numbers: &[51],
        },
        Case {
            name: "the URL of a pull request of the repository",
            body: "## Blocked by\n\n- https://github.com/timmattison/example/pull/52\n",
            numbers: &[52],
        },
        Case {
            name: "the URL of an issue in an autolink",
            body: "## Blocked by\n\n- <https://github.com/timmattison/example/issues/51>\n",
            numbers: &[51],
        },
        Case {
            name: "the URL of an issue in another case, with a fragment",
            body: "## Blocked by\n\n- https://github.com/TimMattison/Example/issues/51#issuecomment-7\n",
            numbers: &[51],
        },
        Case {
            name: "a bold label with the URL of an issue and a number",
            body: "**Blocked by:** https://github.com/timmattison/example/issues/51 and #52\n",
            numbers: &[51, 52],
        },
        Case {
            name: "a nested list item indented by four spaces",
            body: "## Blocked by\n\n- The solver\n    - #168\n",
            numbers: &[168],
        },
        Case {
            name: "a nested list item after a blank line",
            body: "## Blocked by\n\n- The solver\n\n    - #168\n",
            numbers: &[168],
        },
        Case {
            name: "a nested list item after a blank line, indented by a tab",
            body: "## Blocked by\n\n- The solver\n\n\t- #168\n",
            numbers: &[168],
        },
        Case {
            name: "a paragraph inside a list item after a blank line",
            body: "## Blocked by\n\n- The solver\n\n    #9\n",
            numbers: &[9],
        },
        Case {
            name: "indented code inside a list item after a blank line",
            body: "## Blocked by\n\n- The solver\n\n      #9\n",
            numbers: &[],
        },
        Case {
            name: "indented code after prose that ends the list",
            body: "## Blocked by\n\n- #5\n\nSome prose.\n\n    #9\n",
            numbers: &[5],
        },
        Case {
            name: "a paragraph of the outer item after a nested item",
            body: "## Blocked by\n\n- A\n\n    - #6\n\n  #7\n",
            numbers: &[6, 7],
        },
        Case {
            name: "a code fence inside a list item after a blank line",
            body: "## Blocked by\n\n- The solver\n\n    ```\n    - #9\n    ```\n\n    #11\n",
            numbers: &[11],
        },
        Case {
            name: "an HTML comment inside a list item after a blank line",
            body: "## Blocked by\n\n- The solver\n\n    <!--\n    - #9\n    -->\n\n    #11\n",
            numbers: &[11],
        },
        Case {
            name: "indented code after a list that a block quote holds",
            body: "## Blocked by\n\n> - #5\n\n    #9\n",
            numbers: &[5],
        },
        Case {
            name: "indented code after a block quote that ends the list",
            body: "## Blocked by\n\n- #5\n\n> quote\n\n    #9\n",
            numbers: &[5],
        },
        Case {
            name: "a subheading inside the section",
            body: "## Blocked by\n\n### Before the solver\n\n- #11\n",
            numbers: &[11],
        },
        Case {
            name: "a subheading that names a number inside the section",
            body: "## Blocked by\n\n### #11 (the solver)\n\n- #12\n",
            numbers: &[11, 12],
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
        Case {
            name: "a section inside an HTML comment",
            body: "<!--\n## Blocked by\n\n- #31\n-->\n\nReal text.\n",
            numbers: &[],
        },
        Case {
            name: "an HTML comment on one line under the heading",
            body: "## Blocked by\n\n<!-- - #9 -->\n- #11\n",
            numbers: &[11],
        },
        Case {
            name: "an HTML comment that closes before the section goes on",
            body: "## Blocked by\n\n<!--\n- #9\n-->\n- #11\n",
            numbers: &[11],
        },
        Case {
            name: "an HTML comment that nothing closes",
            body: "## Blocked by\n\n- #11\n\n<!--\n- #12\n",
            numbers: &[11],
        },
        Case {
            name: "an HTML comment that interrupts a list item",
            body: "## Blocked by\n\n- The solver\n<!-- it lands first -->\n#12\n",
            numbers: &[12],
        },
        Case {
            name: "an HTML comment after a number in the same item",
            body: "## Blocked by\n\n- #5 <!-- was #4 -->\n",
            numbers: &[5],
        },
        Case {
            name: "an HTML comment inside a code fence opens none",
            body: "## Blocked by\n\n```\n<!--\n```\n\n- #11\n",
            numbers: &[11],
        },
        Case { name: "a Blocks heading, which is the other direction", body: "## Blocks\n\n- #12\n", numbers: &[] },
        Case { name: "a Blocks heading that names a number", body: "## Blocks #12\n", numbers: &[] },
        Case { name: "a heading that starts with a number, outside any section", body: "## #41 lands first\n", numbers: &[] },
        Case {
            name: "a heading of the same level ends the section",
            body: "## Blocked by\n\n- #11\n\n## Acceptance criteria\n\n- #12 still passes\n",
            numbers: &[11],
        },
        Case {
            name: "a heading that ends the section and names a number",
            body: "## Blocked by\n\n- #11\n\n## Notes on #12\n",
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
            name: "the URL of an issue of another repository",
            body: "## Blocked by\n\n- https://github.com/timmattison/muxiavelli/issues/294\n- #5\n",
            numbers: &[5],
        },
        Case {
            name: "the URL of an issue of another repository before a comma",
            body: "## Blocked by\n\n- https://github.com/timmattison/muxiavelli/issues/294, #5\n",
            numbers: &[5],
        },
        Case {
            name: "the URL of a file of the repository",
            body: "## Blocked by\n\n- https://github.com/timmattison/example/blob/main/README.md\n",
            numbers: &[],
        },
        Case {
            name: "the URL of an issue whose number is zero",
            body: "## Blocked by\n\n- https://github.com/timmattison/example/issues/0\n",
            numbers: &[],
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
        Case { name: "tildes that follow a space close no strike", body: "## Blocked by\n\n- ~~#21 ~~ #22\n", numbers: &[] },
        Case { name: "a tilde that a space follows opens no strike", body: "## Blocked by\n\n- ~ #21~ #22\n", numbers: &[] },
        Case {
            name: "a strike goes on past tildes that follow a space",
            body: "## Blocked by\n\n- ~~#21 ~~ #22~~ #23\n",
            numbers: &[23],
        },
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
            name: "a phrase between two tildes that mean approximately",
            body: "This takes ~2h. It is blocked by #7. It saves ~30% of the time.\n",
            numbers: &[],
        },
        Case {
            name: "a phrase between two tildes with a space on each side",
            body: "The fix takes 2 ~ 3 days. It is blocked by #9. The test takes 1 ~ 2 days.\n",
            numbers: &[],
        },
        Case {
            name: "a line of a tracker that says what blocks another issue",
            body: "- [ ] #12 \u{2014} Click to open *blocked by #11*\n",
            numbers: &[],
        },
    ];

    /// The repository every case of [`CASES`] is the body of an issue of.
    const REPO: &str = "timmattison/example";

    #[test]
    fn every_form_of_a_blocker_reads_as_its_case_says() {
        let repo = Repo::parse(REPO).expect("the repository of the cases is a repository");
        let wrong: Vec<String> = CASES
            .iter()
            .filter_map(|case| {
                let read: Vec<u64> = read(case.body, &repo)
                    .iter()
                    .map(|number| number.get())
                    .collect();
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
