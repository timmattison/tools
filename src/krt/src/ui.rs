//! The printed view of the aggregate table, and the measure it stands on.
//!
//! A terminal holds columns, and a string holds bytes. The two counts agree
//! only while every character of the text is an ASCII one. One character of a
//! host name takes one column, or two columns when the glyph is a wide one, and
//! a name of two bytes for each character takes half the bytes it looks like it
//! takes. A cell that measured its text in bytes would therefore print a short
//! name over the column that follows it, and a cut by bytes would stop in the
//! middle of a character and panic. This module measures in columns and cuts on
//! a character, so every cell of the table keeps its column and every name
//! stays a string.
//!
//! A later slice prints the table on these helpers, and states there the
//! order in which the columns drop as the terminal gets narrow.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the render of the table is the reader of these helpers, and that render arrives in a later slice of issue #370, so the tests of this module read them today"
    )
)]

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The number of terminal columns that the text occupies.
///
/// A wide glyph, as one of the Japanese or the emoji ones, takes two columns.
/// Every other printable character takes one, and a character that prints
/// nothing takes none.
pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// The text, cut to at most `width` terminal columns.
///
/// Text that already fits comes back unchanged. Text that does not fit loses
/// its tail, and the cut falls between two characters, never inside one. A wide
/// glyph that would take the one column past the limit goes away whole, so the
/// result never runs one column over the width it was given. A width of zero
/// gives an empty string.
///
/// The cut carries no ellipsis. The Host column of the table is narrow, and
/// three of its columns are three columns of the name. A name that lost its
/// tail already reads as a name that lost its tail.
///
/// This is not `termbar::truncate_filename`. That helper keeps the extension of
/// a file name and cuts the middle, because the extension of a file names the
/// kind of the file. A host name is not a file name: the tail of
/// `ae-1.core.example.net` is the domain that every router of that network
/// shares, and the head is the one part that tells the routers apart. The
/// helper would therefore keep the part that says the least.
pub(crate) fn truncate_to_width(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    let mut taken = 0;
    for character in text.chars() {
        // A character that prints nothing takes no column, and it stays with
        // the character it belongs to.
        let columns = UnicodeWidthChar::width(character).unwrap_or(0);
        if taken + columns > width {
            // The loop stops here, and it does not look for a narrow character
            // behind this one. A cut that kept a later character would print a
            // name that the path never held.
            break;
        }
        taken += columns;
        kept.push(character);
    }
    kept
}

/// The recent round-trip times of one key, as a bar for each of them.
pub(crate) fn sparkline(_samples: impl ExactSizeIterator<Item = f64>, _width: usize) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::{display_width, sparkline, truncate_to_width};

    /// A name of wide glyphs. Each of the eight characters takes two columns,
    /// so the name takes 16 columns and 24 bytes.
    const JAPANESE: &str = "日本語のホスト名";

    /// A name of three emoji and three wide Japanese characters. Each of the
    /// six characters takes two columns, so the name takes 12 columns.
    const EMOJI: &str = "🎉🎊🎁ホスト";

    /// A name whose fourth character takes two bytes and one column.
    ///
    /// A cut by bytes at the fourth byte lands inside the `é` and panics. A cut
    /// by characters at the fourth character gives `café`.
    const ACCENTED: &str = "café-router.example";

    /// A name of ASCII characters, one column for each byte.
    const ASCII: &str = "router.lan";

    #[test]
    fn the_width_of_an_ascii_name_is_the_count_of_its_characters() {
        assert_eq!(
            display_width(ASCII),
            10,
            "each of the ten ASCII characters takes one column"
        );
    }

    #[test]
    fn the_width_of_a_wide_name_is_two_columns_for_each_glyph() {
        assert_eq!(
            display_width(JAPANESE),
            16,
            "each of the eight glyphs takes two columns"
        );
        assert_eq!(
            display_width(EMOJI),
            12,
            "each of the six glyphs takes two columns"
        );
    }

    #[test]
    fn the_width_of_a_mixed_name_counts_the_bytes_of_no_character() {
        // The name holds 19 characters, and the `é` holds two bytes. A measure
        // in bytes would give 20.
        assert_eq!(
            display_width(ACCENTED),
            19,
            "an accented character takes one column and two bytes"
        );
        assert_eq!(
            display_width("ttl 日本"),
            8,
            "four ASCII characters and two wide glyphs take 4 + 4 columns"
        );
    }

    #[test]
    fn a_name_that_fits_comes_back_whole() {
        assert_eq!(
            truncate_to_width(ASCII, 30),
            ASCII,
            "a name below the width keeps every character"
        );
        assert_eq!(
            truncate_to_width(ASCII, 10),
            ASCII,
            "a name of exactly the width keeps every character"
        );
        assert_eq!(
            truncate_to_width(JAPANESE, 16),
            JAPANESE,
            "a wide name of exactly the width keeps every glyph"
        );
    }

    #[test]
    fn a_width_of_zero_gives_an_empty_name() {
        for text in [ASCII, JAPANESE, EMOJI, ACCENTED] {
            assert_eq!(
                truncate_to_width(text, 0),
                "",
                "no column holds no character of {text}"
            );
        }
    }

    #[test]
    fn a_wide_glyph_that_crosses_the_limit_goes_away_whole() {
        // Two glyphs take four columns, and the third would take the fifth and
        // the sixth. A limit of five therefore holds two of the glyphs.
        assert_eq!(
            truncate_to_width(JAPANESE, 5),
            "日本",
            "the glyph that would cross the limit goes away whole"
        );
        assert_eq!(
            display_width(&truncate_to_width(JAPANESE, 5)),
            4,
            "the cut name stops one column short of the odd limit"
        );
        assert_eq!(
            truncate_to_width(JAPANESE, 6),
            "日本語",
            "an even limit holds three of the glyphs"
        );
    }

    #[test]
    fn an_emoji_name_cuts_on_a_glyph() {
        // Three emoji take six columns, and the fourth glyph would take the
        // seventh and the eighth.
        assert_eq!(
            truncate_to_width(EMOJI, 7),
            "🎉🎊🎁",
            "the wide glyph that would cross the limit goes away whole"
        );
        assert_eq!(
            truncate_to_width(EMOJI, 2),
            "🎉",
            "two columns hold one emoji"
        );
        assert_eq!(
            truncate_to_width(EMOJI, 1),
            "",
            "one column holds no wide glyph"
        );
    }

    #[test]
    fn an_accented_name_cuts_on_a_character_and_not_on_a_byte() {
        // The fourth character is the `é`, and it holds two bytes. A cut at the
        // fourth byte lands inside it.
        assert_eq!(
            truncate_to_width(ACCENTED, 4),
            "café",
            "the cut keeps the whole of the accented character"
        );
        assert_eq!(
            truncate_to_width(ACCENTED, 3),
            "caf",
            "the cut before the accented character keeps three characters"
        );
        assert_eq!(
            truncate_to_width(ACCENTED, 12),
            "café-router.",
            "the cut counts the accented character as one column"
        );
    }

    #[test]
    fn a_cut_name_never_runs_over_the_width() {
        for text in [ASCII, JAPANESE, EMOJI, ACCENTED, "日ab🎉c"] {
            for width in 0..=display_width(text) + 2 {
                let cut = truncate_to_width(text, width);
                assert!(
                    display_width(&cut) <= width,
                    "the cut of {text} to {width} columns takes {} of them",
                    display_width(&cut)
                );
                assert!(
                    text.starts_with(&cut),
                    "the cut of {text} to {width} columns keeps its head"
                );
            }
        }
    }

    #[test]
    fn a_cut_name_keeps_as_many_columns_as_it_holds() {
        // The name takes seven columns: two for the wide glyph, one for each of
        // the two ASCII letters, two for the emoji, and one for the last
        // letter. A limit of six therefore holds every character but the last
        // one.
        let text = "日ab🎉c";
        assert_eq!(display_width(text), 7, "the name takes seven columns");
        assert_eq!(
            truncate_to_width(text, 6),
            "日ab🎉",
            "the cut keeps every character that fits"
        );
        assert_eq!(
            truncate_to_width(text, 4),
            "日ab",
            "the emoji would take the fifth and the sixth column"
        );
    }

    /// The eight bars of the sparkline, lowest first.
    ///
    /// The test states them, and the module states them again. The two are on
    /// purpose: a test that read the constant of the module would agree with
    /// every set of glyphs the module ever holds, and the set of glyphs is the
    /// part of the sparkline a reader of the table sees.
    const BARS: &str = "▁▂▃▄▅▆▇█";

    /// The bar of a set of samples, at a width.
    fn bar(samples: &[f64], width: usize) -> String {
        sparkline(samples.iter().copied(), width)
    }

    #[test]
    fn a_rising_ramp_draws_every_bar_of_the_set() {
        // The samples run from 1 to 8, so the span is 7. The bar of a sample is
        // its distance from the smallest one, over the span, times the eight
        // bars: 0/7, 8/7, 16/7 ... which cut to 0, 1, 2, 3, 4, 5, 6, and the
        // largest sample gives 8, which the clamp puts on the last bar.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0], 9),
            BARS,
            "eight samples one step apart draw each of the eight bars once"
        );
    }

    #[test]
    fn the_smallest_sample_takes_the_lowest_bar_and_the_largest_takes_the_highest() {
        // The samples run from 10 to 40, so the span is 30. The bar of 20 is
        // (20 - 10) / 30 * 8 = 2.67, which cuts to the third bar. The bar of 30
        // is (30 - 10) / 30 * 8 = 5.33, which cuts to the sixth bar.
        let drawn = bar(&[10.0, 20.0, 30.0, 40.0], 9);
        assert_eq!(drawn, "▁▃▆█", "the middle samples take the middle bars");
        assert_eq!(
            drawn.chars().next(),
            Some('▁'),
            "the smallest sample takes the lowest bar"
        );
        assert_eq!(
            drawn.chars().next_back(),
            Some('█'),
            "the largest sample takes the highest bar"
        );
    }

    #[test]
    fn no_sample_draws_nothing() {
        assert_eq!(bar(&[], 9), "", "a key with no round-trip time draws no bar");
    }

    #[test]
    fn a_width_of_zero_draws_nothing() {
        assert_eq!(
            bar(&[1.0, 2.0, 3.0], 0),
            "",
            "no column holds no bar, however many samples the key holds"
        );
    }

    #[test]
    fn samples_that_are_all_equal_draw_the_lowest_bar() {
        // The smallest sample and the largest one are the same, so the span is
        // zero and no sample stands above another one. A flat line at the floor
        // says that.
        assert_eq!(
            bar(&[5.0, 5.0, 5.0], 9),
            "▁▁▁",
            "a key whose round-trip time never moved draws a flat line"
        );
        assert_eq!(
            bar(&[0.0, 0.0], 9),
            "▁▁",
            "a span of zero at the floor draws the lowest bar as well"
        );
    }

    #[test]
    fn the_bar_holds_the_last_samples_of_a_longer_history() {
        // The first two samples are far above the last four. A window of four
        // therefore drops them, and the scale of the window runs from 1 to 4:
        // (2 - 1) / 3 * 8 = 2.67 cuts to the third bar, and (3 - 1) / 3 * 8 =
        // 5.33 cuts to the sixth.
        let history = [100.0, 200.0, 1.0, 2.0, 3.0, 4.0];
        assert_eq!(
            bar(&history, 4),
            "▁▃▆█",
            "the window holds the last four samples, and its scale reads only them"
        );

        // The same history at a width that holds all of it reads a scale from 1
        // to 200, and the four small samples then crowd on the lowest bar. The
        // two results differ, so the window drops the oldest samples and not
        // the most recent ones.
        assert_eq!(
            bar(&history, 6),
            "▄█▁▁▁▁",
            "a window that holds the whole history reads the large samples too"
        );
    }

    #[test]
    fn one_sample_draws_one_bar() {
        // One sample is the smallest and the largest at once, so the span is
        // zero and the rule of the flat line gives the lowest bar.
        assert_eq!(bar(&[42.0], 9), "▁", "one round-trip time draws one bar");
    }

    #[test]
    fn a_sample_that_is_not_a_number_keeps_the_bar_of_every_other_sample() {
        // A sample that does not compare takes the lowest bar and stays out of
        // the scale. The ramp of eight therefore keeps each of its bars.
        assert_eq!(
            bar(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, f64::NAN], 9),
            "▁▂▃▄▅▆▇█▁",
            "the sample that is not a number draws the lowest bar and moves no other bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NAN, 8.0], 9),
            "▁▁█",
            "the smallest and the largest sample keep their bars around a sample that is not a number"
        );
        assert_eq!(
            bar(&[1.0, 8.0], 9),
            "▁█",
            "the same two samples without it draw the same two bars"
        );
        assert_eq!(
            bar(&[f64::NAN, f64::NAN], 9),
            "▁▁",
            "a window of samples that none of them compare draws a flat line"
        );
        assert_eq!(
            bar(&[1.0, f64::INFINITY, 8.0], 9),
            "▁▁█",
            "an infinity does not compare either, and it takes the lowest bar"
        );
        assert_eq!(
            bar(&[1.0, f64::NEG_INFINITY, 8.0], 9),
            "▁▁█",
            "an infinity below zero takes the lowest bar and holds the scale off the floor"
        );
    }

    #[test]
    fn the_bar_holds_no_character_outside_the_set() {
        let drawn = bar(&[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, f64::NAN], 9);
        assert_eq!(drawn.chars().count(), 9, "one bar stands for one sample");
        for character in drawn.chars() {
            assert!(
                BARS.contains(character),
                "the bar {drawn} holds {character}, which is not one of the eight block elements"
            );
        }
        assert!(
            !drawn.is_ascii(),
            "the sparkline has no ASCII fallback, so no bar of {drawn} is an ASCII character"
        );
    }

    #[test]
    fn the_bar_takes_one_column_for_each_sample_it_draws() {
        // A block element takes one terminal column, so the Recent column holds
        // as many samples as it is wide.
        let history = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0, 5.0, 3.0, 5.0];
        for width in 0..=history.len() + 2 {
            let drawn = bar(&history, width);
            assert_eq!(
                display_width(&drawn),
                width.min(history.len()),
                "a width of {width} over {} samples draws {drawn}",
                history.len()
            );
        }
    }
}
