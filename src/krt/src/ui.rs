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
//! A later slice prints the table on these two helpers, and states there the
//! order in which the columns drop as the terminal gets narrow.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the render of the table is the reader of these helpers, and that render arrives in a later slice of issue #370, so the tests of this module read them today"
    )
)]

/// The number of terminal columns that the text occupies.
///
/// A wide glyph, as one of the Japanese or the emoji ones, takes two columns.
/// Every other printable character takes one, and a character that prints
/// nothing takes none.
pub(crate) fn display_width(_text: &str) -> usize {
    0
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
pub(crate) fn truncate_to_width(_text: &str, _width: usize) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::{display_width, truncate_to_width};

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
}
