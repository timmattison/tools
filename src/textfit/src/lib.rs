//! The one entrance that fits a string into a given number of terminal
//! columns.
//!
//! A tool that prints a table has one rule to obey: no row wraps. A row that
//! wraps costs two lines, and the second line carries no header, so the reader
//! reads a column that is not there. To obey the rule, the tool must cut a
//! string to a budget and pad a string to a width.
//!
//! Neither operation is one line of code. A budget is a count of columns, and
//! a string holds characters. The two counts differ: `日` is one character and
//! two columns, a combining accent is one character and no columns, and a byte
//! is neither. So every function here spends the budget in columns, and it
//! walks the string by character. That is also why none of them can index the
//! string by byte. `&s[..n]` panics the moment `n` lands inside a character,
//! and a process name or a branch name carries such characters.
//!
//! # Why the answers stand in one crate
//!
//! `gsw` wrote all of this first, and it wrote it in its own binary crate. A
//! binary crate builds a program and it builds no library, so no other tool of
//! this workspace can read one line of it. `wn` prints a title beside an issue
//! number, and it must cut that title to the width of the terminal, so `wn`
//! needs the same answers. The only way to reach them was to write them a
//! second time.
//!
//! A second copy is worse than the one it replaces. The two agree on the easy
//! input and they part company at the edges, which is where the whole of the
//! correctness sits: the budget of no columns, the budget of one column that
//! the marker alone fills, the wide character that straddles the last column,
//! and the character of no width that lets two walks over one string pass each
//! other. A test that notices such a difference is the test that fails once a
//! year.
//!
//! # A marker costs a column
//!
//! Each `truncate_*` function marks the cut with `…`, and that marker occupies
//! one column of the budget. [`truncate_right`] therefore answers a budget of
//! no columns with the marker alone, which is one column too wide. That answer
//! is deliberate, and it is the reason [`truncate_to_budget`] stands beside it:
//! a caller that can be handed a zero calls that one instead, and gets an empty
//! string.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The marker that stands where characters were removed.
const ELLIPSIS: char = '…';

/// The number of columns [`ELLIPSIS`] occupies.
const ELLIPSIS_WIDTH: usize = 1;

/// Pad `s` on the right with spaces until its display width reaches `width`.
///
/// A string that is already `width` columns wide, or wider, comes back
/// unchanged. This function never cuts, so a caller that must not overflow
/// cuts first with one of the `truncate_*` functions.
#[must_use]
pub fn pad_right(s: &str, width: usize) -> String {
    let current = UnicodeWidthStr::width(s);
    if current >= width {
        s.to_string()
    } else {
        let mut result = String::with_capacity(s.len() + (width - current));
        result.push_str(s);
        for _ in 0..(width - current) {
            result.push(' ');
        }
        result
    }
}

/// Truncate `s` from the right to fit within `max_width` display columns,
/// suffixing with `…` when truncation happens. UTF-8 safe.
///
/// A budget of no columns gives the marker alone, which is one column wider
/// than the budget. Call [`truncate_to_budget`] when the budget can be zero.
#[must_use]
pub fn truncate_right(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let target = max_width.saturating_sub(ELLIPSIS_WIDTH);
    let mut acc = 0_usize;
    let mut result = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if acc + cw > target {
            break;
        }
        acc += cw;
        result.push(c);
    }
    result.push(ELLIPSIS);
    result
}

/// Truncate `s` from the left to fit within `max_width` display columns,
/// prefixing with `…` when truncation happens. UTF-8 safe.
///
/// A path carries the part that identifies it at the end, so a path that must
/// lose columns loses them from the front.
#[must_use]
pub fn truncate_left(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let target = max_width.saturating_sub(ELLIPSIS_WIDTH);
    let chars: Vec<char> = s.chars().collect();
    let mut acc = 0_usize;
    let mut start = chars.len();
    for (i, c) in chars.iter().enumerate().rev() {
        let cw = UnicodeWidthChar::width(*c).unwrap_or(0);
        if acc + cw > target {
            break;
        }
        acc += cw;
        start = i;
    }
    let mut result = String::from(ELLIPSIS);
    for c in &chars[start..] {
        result.push(*c);
    }
    result
}

/// Truncate `s` to `max_width` display columns by dropping from the middle,
/// joining the head and tail that survive with `…`. Branch names share long
/// prefixes (`feature/…`, `origin/…`) and carry the part that identifies them
/// at the end, so keeping both ends beats keeping either one alone. UTF-8
/// safe: budgets are spent in display columns, never bytes.
#[must_use]
pub fn truncate_middle(s: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let Some(keep) = max_width.checked_sub(ELLIPSIS_WIDTH) else {
        // Not even room for the marker.
        return String::new();
    };
    // The odd column, if there is one, goes to the head: `feature/` prefixes
    // are what the eye lands on first.
    let head_budget = keep.div_ceil(2);
    let tail_budget = keep - head_budget;

    let mut head = String::new();
    let mut spent = 0_usize;
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if spent + cw > head_budget {
            break;
        }
        spent += cw;
        head.push(c);
    }
    let chars: Vec<char> = s.chars().collect();
    let mut tail_start = chars.len();
    let mut spent = 0_usize;
    for (i, c) in chars.iter().enumerate().rev() {
        let cw = UnicodeWidthChar::width(*c).unwrap_or(0);
        if spent + cw > tail_budget {
            break;
        }
        spent += cw;
        tail_start = i;
    }
    // Zero-width characters cost nothing, so both walks can run past each
    // other and duplicate the middle. Keep the halves disjoint.
    let tail_start = tail_start.max(head.chars().count());

    let mut result = head;
    result.push(ELLIPSIS);
    result.extend(&chars[tail_start..]);
    result
}

/// [`truncate_right`], but a zero budget yields nothing rather than a lone
/// `…` — which would itself be one column too wide.
#[must_use]
pub fn truncate_to_budget(s: &str, budget: usize) -> String {
    if budget == 0 {
        String::new()
    } else {
        truncate_right(s, budget)
    }
}

/// Center `text` within `width` display columns, padding with spaces.
///
/// A string that already fills the width, or overflows it, comes back
/// unchanged.
#[must_use]
pub fn center(text: &str, width: usize) -> String {
    let text_w = UnicodeWidthStr::width(text);
    if text_w >= width {
        return text.to_string();
    }
    let total_pad = width - text_w;
    let left = total_pad / 2;
    let right = total_pad - left;
    let mut result = String::with_capacity(width);
    for _ in 0..left {
        result.push(' ');
    }
    result.push_str(text);
    for _ in 0..right {
        result.push(' ');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_middle_keeps_both_ends_and_fits_the_budget() {
        assert_eq!(truncate_middle("feature/topic", 20), "feature/topic");
        // 18 columns cut to 13: head gets 6, tail gets 6, `…` gets 1.
        assert_eq!(truncate_middle("feature/some-topic", 13), "featur…-topic");
        // One column less: head keeps 6, tail keeps 5.
        assert_eq!(truncate_middle("feature/some-topic", 12), "featur…topic");
    }

    #[test]
    fn truncate_middle_counts_columns_not_characters() {
        // Each of these CJK characters is one character and two columns. A
        // function that counts characters gives back double the budget it got,
        // and a function that cuts the string by byte panics in the middle of
        // a character.
        let name = "日本語のとても長い名前";
        for budget in 0..=UnicodeWidthStr::width(name) + 2 {
            let cut = truncate_middle(name, budget);
            assert!(
                UnicodeWidthStr::width(cut.as_str()) <= budget,
                "budget {budget} produced {cut:?} ({} columns)",
                UnicodeWidthStr::width(cut.as_str()),
            );
        }
    }

    #[test]
    fn truncate_middle_degrades_to_nothing_before_overflowing() {
        // One column holds the marker alone, and no column holds nothing.
        assert_eq!(truncate_middle("feature/topic", 1), "…");
        assert_eq!(truncate_middle("feature/topic", 0), "");
    }

    #[test]
    fn truncate_right_keeps_the_head_and_marks_the_cut() {
        assert_eq!(truncate_right("a title", 20), "a title");
        assert_eq!(truncate_right("a longer title", 8), "a longe…");
    }

    #[test]
    fn truncate_right_never_splits_a_wide_character() {
        // The budget of 4 holds one wide character and the marker. It does not
        // hold two wide characters, and half of one is not an answer.
        assert_eq!(truncate_right("日本語", 4), "日…");
        assert_eq!(truncate_right("日本語", 3), "日…");
    }

    #[test]
    fn truncate_to_budget_gives_nothing_for_no_columns() {
        // `truncate_right` gives the marker here, and the marker is one column
        // wide. That is the whole reason this function stands beside it.
        assert_eq!(truncate_right("a title", 0), "…");
        assert_eq!(truncate_to_budget("a title", 0), "");
        assert_eq!(truncate_to_budget("a title", 20), "a title");
    }

    #[test]
    fn truncate_left_keeps_the_tail() {
        assert_eq!(
            truncate_left("src/wn/src/main.rs", 30),
            "src/wn/src/main.rs"
        );
        assert_eq!(truncate_left("src/wn/src/main.rs", 10), "…c/main.rs");
    }

    #[test]
    fn pad_right_measures_columns_and_never_cuts() {
        assert_eq!(pad_right("ab", 5), "ab   ");
        // Two characters and four columns, so one space fills the width.
        assert_eq!(pad_right("日本", 5), "日本 ");
        assert_eq!(pad_right("a longer string", 3), "a longer string");
    }

    #[test]
    fn center_splits_the_remainder_with_the_left_side_first() {
        assert_eq!(center("bin", 7), "  bin  ");
        assert_eq!(center("bin", 8), "  bin   ");
        assert_eq!(center("bin", 3), "bin");
    }
}
