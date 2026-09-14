//! The spans of text that tildes strike through.
//!
//! GitHub draws a line through the text between `~~` and `~~`, and through the
//! text between `~` and `~`. An author strikes a blocker through, as in
//! `~~#21~~`, to take it back. So the reader of `Blocked by` reads past a span
//! struck through, and it must strike through exactly where GitHub does. A span
//! that only the reader strikes takes away a blocker that stands. A span that
//! only GitHub strikes keeps a blocker that the author took back.
//!
//! `fixtures/github-strikes.tsv` holds the HTML that GitHub made of each case of
//! a corpus, and the tests of this module hold the rule to it.
//! `scripts/record-github-strikes.ts` records that file.

/// The most tildes that open a span struck through. One more opens a code fence
/// at the start of a line, and it strikes nothing inside a block.
pub(crate) const MAX_STRIKE_MARKS: usize = 2;

/// The text after the span struck through that `text` opens with, or `None`
/// when it opens none.
///
/// One or two tildes open the span, as GitHub renders both `~#21~` and
/// `~~#21~~`, but only when a character follows them and that character is not
/// space. A later run of the same number of tildes closes the span only when the
/// character in front of that run is not space. A run that cannot close is text,
/// and the search goes on past it, so `~2h` and `~30%` strike nothing. A span
/// that nothing closes is no strike, and the tildes stay in front of the text.
///
/// Space is what [`is_space`] reads.
pub(crate) fn after_strike(text: &str) -> Option<&str> {
    let inside = text.trim_start_matches('~');
    let marks = text.len() - inside.len();
    let opens = inside.chars().next().is_some_and(|c| !is_space(c));
    if !(1..=MAX_STRIKE_MARKS).contains(&marks) || !opens {
        return None;
    }
    let mut rest = inside;
    loop {
        let at = rest.find('~')?;
        let before = rest.get(..at)?.chars().next_back();
        let run = rest.get(at..)?;
        let after = run.trim_start_matches('~');
        if run.len() - after.len() == marks && !before.is_some_and(is_space) {
            return Some(after);
        }
        rest = after;
    }
}

/// Whether GitHub reads `c` as space next to a run of tildes.
///
/// The set is the set of `cmark_utf8proc_is_space` in `src/utf8.c` of
/// cmark-gfm, the parser of GitHub: tab, line feed, form feed, carriage return,
/// and every character of the Unicode category Zs. Zs holds space, no-break
/// space, U+1680, U+2000 to U+200A, U+202F, U+205F, and U+3000.
///
/// This set is smaller than the White_Space property of Unicode, which
/// [`char::is_whitespace`] reads. White_Space also holds U+000B, U+0085, U+2028,
/// and U+2029. GitHub does not read those as space, so a run of tildes next to
/// one of them can open or close a strike.
fn is_space(c: char) -> bool {
    let control = matches!(c, '\t' | '\n' | '\u{C}' | '\r');
    let zs = matches!(
        c,
        ' ' | '\u{A0}' | '\u{1680}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
    ) || ('\u{2000}'..='\u{200A}').contains(&c);
    control || zs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The HTML that GitHub made of each case, one case to a line: the case, a
    /// tab, and the HTML inside the paragraph.
    const GITHUB_STRIKES: &str = include_str!("../fixtures/github-strikes.tsv");

    /// The character between the case and the HTML on a line of the fixture.
    const FIELD_SEPARATOR: char = '\t';

    /// The tag that opens a span struck through.
    const DEL_OPEN: &str = "<del>";

    /// The tag that closes a span struck through.
    const DEL_CLOSE: &str = "</del>";

    /// The most mismatches that the message of a failure shows.
    const SHOWN_MISMATCHES: usize = 40;

    /// The HTML that the reader makes of `case`: the text, with each span that
    /// [`after_strike`] reads put between `<del>` and `</del>`.
    ///
    /// A span starts only where a run of tildes starts. Its opener is that run,
    /// and its closer is the run of the same number of tildes that ends the span.
    fn reader_rendering(case: &str) -> String {
        let mut html = String::new();
        let mut rest = case;
        let mut after_tilde = false;
        while !rest.is_empty() {
            if let Some(after) = (!after_tilde).then(|| after_strike(rest)).flatten() {
                let marks = rest.len() - rest.trim_start_matches('~').len();
                let span = rest
                    .get(..rest.len() - after.len())
                    .expect("the text after a span is the end of the text");
                let inside = span
                    .get(marks..span.len() - marks)
                    .expect("a span holds its opener and its closer");
                html.push_str(DEL_OPEN);
                html.push_str(inside);
                html.push_str(DEL_CLOSE);
                rest = after;
                after_tilde = true;
            } else {
                let mut chars = rest.chars();
                let c = chars.next().expect("the text is not empty");
                html.push(c);
                rest = chars.as_str();
                after_tilde = c == '~';
            }
        }
        html
    }

    /// The reader strikes text through exactly where GitHub strikes it through.
    ///
    /// The fixture is the rendering that GitHub itself made of each case,
    /// recorded by `src/wn/scripts/record-github-strikes.ts`. So this test holds
    /// the rule to GitHub, not to a model of GitHub. A case that you add to
    /// `CASES` in `blocked_by.rs`, and that turns on how tildes pair, belongs in
    /// the recorder too.
    #[test]
    fn strikes_through_where_github_strikes_through() {
        let mut cases: usize = 0;
        let mut mismatches: Vec<String> = Vec::new();
        for line in GITHUB_STRIKES.lines() {
            let (case, github) = line
                .split_once(FIELD_SEPARATOR)
                .unwrap_or_else(|| panic!("the line {line:?} of the fixture holds no tab"));
            cases += 1;
            let reader = reader_rendering(case);
            if reader != github {
                mismatches.push(format!(
                    "{case:?}: GitHub renders {github:?} and the reader renders {reader:?}"
                ));
            }
        }
        assert!(cases > 0, "the fixture holds no case");
        let count = mismatches.len();
        let shown = mismatches
            .iter()
            .take(SHOWN_MISMATCHES)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            mismatches.is_empty(),
            "the reader and GitHub strike through differently in {count} of {cases} cases. \
             The first {SHOWN_MISMATCHES} are:\n{shown}"
        );
    }
}
