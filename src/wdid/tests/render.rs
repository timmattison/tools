//! Tests of the report: the text, the JSON, and the list of unparsed commands.

use wdid::render::table_width;

/// `COLUMNS` wins, then the terminal, else there is no width. A value that is
/// not a positive integer is ignored.
#[test]
fn the_table_width_takes_the_stated_width_then_the_terminal() {
    let cases: [(Option<&str>, Option<u16>, Option<u16>); 10] = [
        (Some("150"), Some(80), Some(150)),
        (Some("150"), None, Some(150)),
        (Some(" 150\n"), Some(80), Some(150)),
        (None, Some(80), Some(80)),
        (None, None, None),
        (Some("0"), Some(80), Some(80)),
        (Some("wide"), Some(80), Some(80)),
        (Some("-5"), None, None),
        (Some("70000"), Some(80), Some(80)),
        (None, Some(0), None),
    ];
    for (stated, terminal, expected) in cases {
        assert_eq!(
            table_width(stated, terminal),
            expected,
            "stated {stated:?}, terminal {terminal:?}"
        );
    }
}
