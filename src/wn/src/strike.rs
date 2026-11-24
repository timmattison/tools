//! The spans of text that tildes strike through.
//!
//! GitHub draws a line through the text between `~~` and `~~`, and through the
//! text between `~` and `~`. An author strikes a blocker through, as in
//! `~~#21~~`, to take it back. So the reader of `Blocked by` reads past a span
//! struck through, and it must strike through where GitHub does. A span that
//! only the reader strikes takes away a blocker that stands. A span that only
//! GitHub strikes keeps a blocker that the author took back.
//!
//! # The rule
//!
//! GitHub renders Markdown with cmark-gfm. [`Strikes::of`] ports how cmark-gfm
//! pairs tildes: `cmark_inline_parser_scan_delimiters` and `process_emphasis`
//! in `src/inlines.c`, and `insert` in `extensions/strikethrough.c`.
//!
//! 1. A run of tildes holds at most [`MAX_RUN_MARKS`] tildes. The next tilde
//!    starts a new run.
//! 2. A run can open when the character after it is not space. It cannot open
//!    when that character is punctuation and the character in front of the run
//!    is neither space nor punctuation. A run can close by the same rule, with
//!    the two sides exchanged. The start and the end of the text count as
//!    space. [`is_space`] and [`is_punctuation`] give the two sets.
//! 3. A run of [`MAX_STRIKE_MARKS`] tildes or fewer that can open or close is a
//!    delimiter. A longer run is text.
//! 4. The delimiters that can close take turns from left to right. Each one
//!    takes the nearest delimiter in front of it that can open. A run of one
//!    tilde and a run of two tildes do not pair when the closer can also open
//!    or the opener can also close.
//! 5. When a closer takes an opener, the two and every delimiter between them
//!    stop being delimiters. The pair strikes through the text between them
//!    only when the two runs hold the same number of tildes. So `~~#21~#22~~`
//!    strikes nothing: `~` closes `~~`, and the last `~~` then finds no opener.
//! 6. A closer that finds no opener sets a floor for its length modulo 3. A
//!    later closer of that length modulo 3 does not look for an opener in front
//!    of the floor.
//!
//! So strikes can nest, and they never cross.
//!
//! # What the model leaves out
//!
//! The model pairs tildes over the text of one block, and it reads no other
//! Markdown. It does not model code spans, backslash escapes, raw HTML, links
//! and autolinks, or the pairing of `*` and `_`. A pair of emphasis marks that
//! GitHub matches removes the tilde delimiters between them. So a tilde inside
//! one of those constructs can pair differently on GitHub.
//!
//! `fixtures/github-strikes.tsv` holds the HTML that GitHub made of each case of
//! a corpus, and the tests of this module hold the rule to it. The corpus leaves
//! out the characters of the constructs above for that reason.
//! `scripts/record-github-strikes.ts` records that file.

use std::cmp::Ordering;

/// The mark that opens and closes a strike.
const TILDE: char = '~';

/// The character that cmark-gfm reads in front of the start of the text and
/// after its end. It is space.
const EDGE: char = '\n';

/// The most tildes that cmark-gfm reads into one run.
const MAX_RUN_MARKS: usize = 100;

/// The most tildes in a run that opens or closes a strike. A run of more
/// tildes is text.
const MAX_STRIKE_MARKS: usize = 2;

/// The modulus of the rule of 3 of CommonMark, which compares the lengths of an
/// opener and a closer.
const RULE_OF_THREE: usize = 3;

/// The strikes that GitHub renders in the text of one block.
pub(crate) struct Strikes<'a> {
    /// The text of the block.
    text: &'a str,
    /// Each strike, in the order of its closer.
    strikes: Vec<Strike>,
}

/// A run of tildes: the byte offset of its first tilde, and its number of
/// tildes. A tilde is one byte, so the run ends at `start + marks`.
#[derive(Clone, Copy)]
struct Run {
    start: usize,
    marks: usize,
}

impl Run {
    /// The byte offset after the last tilde of the run.
    fn end(self) -> usize {
        self.start + self.marks
    }
}

/// A span struck through: the run that opens it and the run that closes it.
struct Strike {
    opener: Run,
    closer: Run,
}

/// A run of tildes that can open a strike, close a strike, or both.
struct Delimiter {
    run: Run,
    can_open: bool,
    can_close: bool,
}

impl<'a> Strikes<'a> {
    /// The strikes that GitHub renders in `text`, the text of one block.
    ///
    /// The module doc gives the rule and the Markdown that it leaves out.
    pub(crate) fn of(text: &'a str) -> Self {
        let delimiters = delimiters_of(text);
        // Whether each delimiter is still a delimiter.
        let mut live = vec![true; delimiters.len()];
        // For each length modulo 3, the first delimiter that a closer of that
        // length can take as its opener.
        let mut floors = [0; RULE_OF_THREE];
        let mut strikes = Vec::new();
        for (at, closer) in delimiters.iter().enumerate() {
            if !closer.can_close {
                continue;
            }
            let class = closer.run.marks % RULE_OF_THREE;
            let opener = (floors[class]..at).rev().find(|&index| {
                let opener = &delimiters[index];
                live[index]
                    && opener.can_open
                    && (!(closer.can_open || opener.can_close)
                        || class == 0
                        || !(opener.run.marks + closer.run.marks).is_multiple_of(RULE_OF_THREE))
            });
            if let Some(index) = opener {
                let opener = &delimiters[index];
                if opener.run.marks == closer.run.marks {
                    strikes.push(Strike {
                        opener: opener.run,
                        closer: closer.run,
                    });
                }
                live[index..=at].fill(false);
            } else {
                floors[class] = at;
                if !closer.can_open {
                    live[at] = false;
                }
            }
        }
        Self { text, strikes }
    }

    /// The text after the strike whose opening run starts where `rest` starts,
    /// or `None` when no strike opens there.
    ///
    /// `rest` is a suffix of the text the strikes were built from. The answer is
    /// `None` for any other text, also for a copy of such a suffix.
    pub(crate) fn after(&self, rest: &'a str) -> Option<&'a str> {
        let at = self.text.len().checked_sub(rest.len())?;
        let suffix = self.text.get(at..)?;
        if !std::ptr::eq(suffix.as_ptr(), rest.as_ptr()) {
            return None;
        }
        let strike = self
            .strikes
            .iter()
            .find(|strike| strike.opener.start == at)?;
        self.text.get(strike.closer.end()..)
    }
}

/// The runs of tildes in `text` that are delimiters, in order.
fn delimiters_of(text: &str) -> Vec<Delimiter> {
    let mut delimiters = Vec::new();
    let mut before = EDGE;
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c != TILDE {
            before = c;
            continue;
        }
        let mut marks = 1;
        while marks < MAX_RUN_MARKS && chars.next_if(|&(_, next)| next == TILDE).is_some() {
            marks += 1;
        }
        let after = chars.peek().map_or(EDGE, |&(_, next)| next);
        let can_open = !is_space(after)
            && !(is_punctuation(after) && !is_space(before) && !is_punctuation(before));
        let can_close = !is_space(before)
            && !(is_punctuation(before) && !is_space(after) && !is_punctuation(after));
        if (can_open || can_close) && marks <= MAX_STRIKE_MARKS {
            delimiters.push(Delimiter {
                run: Run { start, marks },
                can_open,
                can_close,
            });
        }
        before = TILDE;
    }
    delimiters
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

/// Whether GitHub reads `c` as punctuation next to a run of tildes.
///
/// The set is the set of `cmark_utf8proc_is_punctuation` in `src/utf8.c` of
/// cmark-gfm: the ASCII punctuation, and the characters of [`PUNCTUATION`].
fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || PUNCTUATION
            .binary_search_by(|&(low, high)| {
                if high < c {
                    Ordering::Less
                } else if low > c {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .is_ok()
}

/// The characters outside ASCII that GitHub reads as punctuation, as sorted
/// ranges that include their two ends.
///
/// The table is the table of `cmark_utf8proc_is_punctuation` in `src/utf8.c` of
/// cmark-gfm at 499789b49373bfa045d0e7547e5ee63444c77bca. Adjacent code points
/// of that table are one range here. The table comes from an older version of
/// Unicode than the category P of today. So U+2E4F, which Unicode now puts in
/// Po, is not punctuation to GitHub. Symbols such as `€`, `©` and `🚧` are not
/// punctuation either.
const PUNCTUATION: &[(char, char)] = &[
    ('\u{A1}', '\u{A1}'),
    ('\u{A7}', '\u{A7}'),
    ('\u{AB}', '\u{AB}'),
    ('\u{B6}', '\u{B7}'),
    ('\u{BB}', '\u{BB}'),
    ('\u{BF}', '\u{BF}'),
    ('\u{37E}', '\u{37E}'),
    ('\u{387}', '\u{387}'),
    ('\u{55A}', '\u{55F}'),
    ('\u{589}', '\u{58A}'),
    ('\u{5BE}', '\u{5BE}'),
    ('\u{5C0}', '\u{5C0}'),
    ('\u{5C3}', '\u{5C3}'),
    ('\u{5C6}', '\u{5C6}'),
    ('\u{5F3}', '\u{5F4}'),
    ('\u{609}', '\u{60A}'),
    ('\u{60C}', '\u{60D}'),
    ('\u{61B}', '\u{61B}'),
    ('\u{61E}', '\u{61F}'),
    ('\u{66A}', '\u{66D}'),
    ('\u{6D4}', '\u{6D4}'),
    ('\u{700}', '\u{70D}'),
    ('\u{7F7}', '\u{7F9}'),
    ('\u{830}', '\u{83E}'),
    ('\u{85E}', '\u{85E}'),
    ('\u{964}', '\u{965}'),
    ('\u{970}', '\u{970}'),
    ('\u{AF0}', '\u{AF0}'),
    ('\u{DF4}', '\u{DF4}'),
    ('\u{E4F}', '\u{E4F}'),
    ('\u{E5A}', '\u{E5B}'),
    ('\u{F04}', '\u{F12}'),
    ('\u{F14}', '\u{F14}'),
    ('\u{F3A}', '\u{F3D}'),
    ('\u{F85}', '\u{F85}'),
    ('\u{FD0}', '\u{FD4}'),
    ('\u{FD9}', '\u{FDA}'),
    ('\u{104A}', '\u{104F}'),
    ('\u{10FB}', '\u{10FB}'),
    ('\u{1360}', '\u{1368}'),
    ('\u{1400}', '\u{1400}'),
    ('\u{166D}', '\u{166E}'),
    ('\u{169B}', '\u{169C}'),
    ('\u{16EB}', '\u{16ED}'),
    ('\u{1735}', '\u{1736}'),
    ('\u{17D4}', '\u{17D6}'),
    ('\u{17D8}', '\u{17DA}'),
    ('\u{1800}', '\u{180A}'),
    ('\u{1944}', '\u{1945}'),
    ('\u{1A1E}', '\u{1A1F}'),
    ('\u{1AA0}', '\u{1AA6}'),
    ('\u{1AA8}', '\u{1AAD}'),
    ('\u{1B5A}', '\u{1B60}'),
    ('\u{1BFC}', '\u{1BFF}'),
    ('\u{1C3B}', '\u{1C3F}'),
    ('\u{1C7E}', '\u{1C7F}'),
    ('\u{1CC0}', '\u{1CC7}'),
    ('\u{1CD3}', '\u{1CD3}'),
    ('\u{2010}', '\u{2027}'),
    ('\u{2030}', '\u{2043}'),
    ('\u{2045}', '\u{2051}'),
    ('\u{2053}', '\u{205E}'),
    ('\u{207D}', '\u{207E}'),
    ('\u{208D}', '\u{208E}'),
    ('\u{2308}', '\u{230B}'),
    ('\u{2329}', '\u{232A}'),
    ('\u{2768}', '\u{2775}'),
    ('\u{27C5}', '\u{27C6}'),
    ('\u{27E6}', '\u{27EF}'),
    ('\u{2983}', '\u{2998}'),
    ('\u{29D8}', '\u{29DB}'),
    ('\u{29FC}', '\u{29FD}'),
    ('\u{2CF9}', '\u{2CFC}'),
    ('\u{2CFE}', '\u{2CFF}'),
    ('\u{2D70}', '\u{2D70}'),
    ('\u{2E00}', '\u{2E2E}'),
    ('\u{2E30}', '\u{2E42}'),
    ('\u{3001}', '\u{3003}'),
    ('\u{3008}', '\u{3011}'),
    ('\u{3014}', '\u{301F}'),
    ('\u{3030}', '\u{3030}'),
    ('\u{303D}', '\u{303D}'),
    ('\u{30A0}', '\u{30A0}'),
    ('\u{30FB}', '\u{30FB}'),
    ('\u{A4FE}', '\u{A4FF}'),
    ('\u{A60D}', '\u{A60F}'),
    ('\u{A673}', '\u{A673}'),
    ('\u{A67E}', '\u{A67E}'),
    ('\u{A6F2}', '\u{A6F7}'),
    ('\u{A874}', '\u{A877}'),
    ('\u{A8CE}', '\u{A8CF}'),
    ('\u{A8F8}', '\u{A8FA}'),
    ('\u{A92E}', '\u{A92F}'),
    ('\u{A95F}', '\u{A95F}'),
    ('\u{A9C1}', '\u{A9CD}'),
    ('\u{A9DE}', '\u{A9DF}'),
    ('\u{AA5C}', '\u{AA5F}'),
    ('\u{AADE}', '\u{AADF}'),
    ('\u{AAF0}', '\u{AAF1}'),
    ('\u{ABEB}', '\u{ABEB}'),
    ('\u{FD3E}', '\u{FD3F}'),
    ('\u{FE10}', '\u{FE19}'),
    ('\u{FE30}', '\u{FE52}'),
    ('\u{FE54}', '\u{FE61}'),
    ('\u{FE63}', '\u{FE63}'),
    ('\u{FE68}', '\u{FE68}'),
    ('\u{FE6A}', '\u{FE6B}'),
    ('\u{FF01}', '\u{FF03}'),
    ('\u{FF05}', '\u{FF0A}'),
    ('\u{FF0C}', '\u{FF0F}'),
    ('\u{FF1A}', '\u{FF1B}'),
    ('\u{FF1F}', '\u{FF20}'),
    ('\u{FF3B}', '\u{FF3D}'),
    ('\u{FF3F}', '\u{FF3F}'),
    ('\u{FF5B}', '\u{FF5B}'),
    ('\u{FF5D}', '\u{FF5D}'),
    ('\u{FF5F}', '\u{FF65}'),
    ('\u{10100}', '\u{10102}'),
    ('\u{1039F}', '\u{1039F}'),
    ('\u{103D0}', '\u{103D0}'),
    ('\u{1056F}', '\u{1056F}'),
    ('\u{10857}', '\u{10857}'),
    ('\u{1091F}', '\u{1091F}'),
    ('\u{1093F}', '\u{1093F}'),
    ('\u{10A50}', '\u{10A58}'),
    ('\u{10A7F}', '\u{10A7F}'),
    ('\u{10AF0}', '\u{10AF6}'),
    ('\u{10B39}', '\u{10B3F}'),
    ('\u{10B99}', '\u{10B9C}'),
    ('\u{11047}', '\u{1104D}'),
    ('\u{110BB}', '\u{110BC}'),
    ('\u{110BE}', '\u{110C1}'),
    ('\u{11140}', '\u{11143}'),
    ('\u{11174}', '\u{11175}'),
    ('\u{111C5}', '\u{111C8}'),
    ('\u{111CD}', '\u{111CD}'),
    ('\u{11238}', '\u{1123D}'),
    ('\u{114C6}', '\u{114C6}'),
    ('\u{115C1}', '\u{115C9}'),
    ('\u{11641}', '\u{11643}'),
    ('\u{12470}', '\u{12474}'),
    ('\u{16A6E}', '\u{16A6F}'),
    ('\u{16AF5}', '\u{16AF5}'),
    ('\u{16B37}', '\u{16B3B}'),
    ('\u{16B44}', '\u{16B44}'),
    ('\u{1BC9F}', '\u{1BC9F}'),
];

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

    /// The HTML that the reader makes of `case`: the text, with the runs of
    /// tildes of each strike of [`Strikes::of`] replaced by `<del>` and `</del>`.
    fn reader_rendering(case: &str) -> String {
        let mut tags: Vec<(Run, &str)> = Strikes::of(case)
            .strikes
            .iter()
            .flat_map(|strike| [(strike.opener, DEL_OPEN), (strike.closer, DEL_CLOSE)])
            .collect();
        tags.sort_unstable_by_key(|(run, _)| run.start);
        let mut html = String::new();
        let mut at = 0;
        for (run, tag) in tags {
            html.push_str(case.get(at..run.start).expect("a run starts at a tilde"));
            html.push_str(tag);
            at = run.end();
        }
        html.push_str(
            case.get(at..)
                .expect("a run ends before the end of the text"),
        );
        html
    }

    /// The reader strikes text through where GitHub strikes it through.
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

    /// The binary search of [`is_punctuation`] finds a character only in a table
    /// that is sorted, and whose ranges do not overlap.
    #[test]
    fn the_punctuation_table_is_sorted_and_holds_no_overlap() {
        for &(low, high) in PUNCTUATION {
            assert!(low <= high, "the range {low:?} to {high:?} is empty");
            assert!(!low.is_ascii(), "the range {low:?} to {high:?} is ASCII");
        }
        for pair in PUNCTUATION.windows(2) {
            if let [(_, high), (next, _)] = pair {
                assert!(
                    high < next,
                    "the range that ends at {high:?} does not come before {next:?}"
                );
            }
        }
    }

    /// Punctuation is what GitHub reads as punctuation, not the category P of
    /// Unicode today.
    #[test]
    fn punctuation_is_the_punctuation_of_github() {
        for c in [
            '!',
            '~',
            '\u{A1}',
            '\u{2014}',
            '\u{FF01}',
            '\u{FF1F}',
            '\u{1BC9F}',
        ] {
            assert!(is_punctuation(c), "{c:?} is punctuation to GitHub");
        }
        for c in [
            'a',
            '1',
            ' ',
            '\u{A0}',
            '\u{2E4F}',
            '\u{20AC}',
            '\u{A9}',
            '\u{1F6A7}',
        ] {
            assert!(!is_punctuation(c), "{c:?} is not punctuation to GitHub");
        }
    }

    /// cmark-gfm reads at most [`MAX_RUN_MARKS`] tildes into one run, so a
    /// longer row of tildes ends in a run that can strike.
    ///
    /// GitHub renders `x `, 101 tildes, and `a~ b` as `x `, 100 tildes, and
    /// `<del>a</del> b`. With 100 tildes it strikes nothing.
    #[test]
    fn a_row_of_more_tildes_than_one_run_holds_ends_in_a_run_that_strikes() {
        let prefix = "x ";
        let long = format!("{prefix}{}a~ b", "~".repeat(MAX_RUN_MARKS + 1));
        let strikes = Strikes::of(&long);
        let rest = long
            .get(prefix.len() + MAX_RUN_MARKS..)
            .expect("the text holds the row of tildes");
        assert_eq!(strikes.after(rest), Some(" b"));

        let full = format!("{prefix}{}a~ b", "~".repeat(MAX_RUN_MARKS));
        assert!(Strikes::of(&full).strikes.is_empty());
    }

    /// [`Strikes::after`] reads only a suffix of the text of its strikes, and
    /// only where a strike opens.
    #[test]
    fn after_reads_only_a_suffix_where_a_strike_opens() {
        let text = "~~#21~~ #22";
        let strikes = Strikes::of(text);
        assert_eq!(strikes.after(text), Some(" #22"));

        let copy = text.to_string();
        assert_eq!(strikes.after(&copy), None, "a copy is not a suffix");

        let inside = text.get(1..).expect("the text starts with a tilde");
        assert_eq!(strikes.after(inside), None, "no strike opens there");
    }
}
