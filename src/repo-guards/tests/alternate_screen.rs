//! Guard tests for `repo_guards::alternate_screen::audit`.
//!
//! Every test builds a real workspace on disk and feeds it to the real
//! `audit()`: member enumeration, the source walk, the parse, and the matcher
//! together.
//!
//! The fixtures are organised by syntactic form. An entry into the alternate
//! screen arrives as a path with no import, a glob import, a method call, a
//! named type, a raw identifier, or a string literal in one of many escapes.
//! Each form is a separate way for a matcher to go quiet. The negative half
//! matters as much: a comment, a doc comment, a string that only names a
//! command, a leave sequence, and a number are prose or data.
//!
//! This file is in the read set of the guard, so no fixture here may hold a
//! real entry. Each fixture source is a raw string, whose value keeps the
//! backslash as text. A fixture that needs a real ESC or a real U+009B builds
//! its source at run time, and says so.
//!
//! Parallel safety: each fixture lives in its own `tempfile::TempDir`.

use std::fs;
use std::path::Path;

use repo_guards::alternate_screen::{self, AlternateScreenError, EXEMPTIONS};
use tempfile::TempDir;

/// The one file that holds a fixture, unless a test names another.
const FIXTURE_FILE: &str = "src/one/src/lib.rs";

/// The file of the one entrance, as [`EXEMPTIONS`] names it.
const ENTRANCE_FILE: &str = "src/altscreen/src/lib.rs";

/// The spelling that the entry sequence of mode 1049 records.
const MODE_1049: &str = "CSI ?1049 h";

/// Build a workspace on disk holding one member crate, whose `src/lib.rs` is
/// `source`.
fn workspace(source: &str) -> TempDir {
    workspace_at(FIXTURE_FILE, source)
}

/// Build a workspace on disk holding one member crate, with `source` written to
/// `relative`, so a test can place a file where an exemption names it.
fn workspace_at(relative: &str, source: &str) -> TempDir {
    let dir = TempDir::new().expect("a temp dir");
    let root = dir.path();

    write(root, "Cargo.toml", "[workspace]\nmembers = [\"src/*\"]\n");

    let member = Path::new(relative)
        .parent()
        .and_then(Path::parent)
        .expect("a path of the shape src/<crate>/src/<file>.rs");
    write(
        &root.join(member),
        "Cargo.toml",
        "[package]\nname = \"one\"\nedition = \"2021\"\n",
    );
    write(root, relative, source);

    dir
}

/// Write `contents` to `dir/relative`, creating the parent directories.
fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    let parent = path.parent().expect("a path with a parent");
    fs::create_dir_all(parent)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
    fs::write(&path, contents).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// Audit a workspace whose one file is `source`, and return the spellings that
/// the audit reports for it, sorted.
fn reported(source: &str) -> Vec<String> {
    let dir = workspace(source);
    let report = alternate_screen::audit(dir.path()).expect("the audit reaches a verdict");
    assert!(report.files_examined() > 0, "the audit read nothing");
    report
        .offenders()
        .iter()
        .flat_map(|offender| offender.spellings().to_vec())
        .collect()
}

/// Assert that `source` reports exactly `expected`.
fn assert_reports(source: &str, expected: &[&str]) {
    assert_eq!(reported(source), expected, "for the source:\n{source}");
}

/// Assert that `source` reports nothing.
fn assert_clean(source: &str) {
    assert_reports(source, &[]);
}

// ---------------------------------------------------------------------------
// Positive: identifiers
// ---------------------------------------------------------------------------

#[test]
fn reports_a_crossterm_path_with_no_import() {
    assert_reports(
        r#"fn f(out: &mut Vec<u8>) {
    execute!(out, crossterm::terminal::EnterAlternateScreen);
}"#,
        &["EnterAlternateScreen"],
    );
}

#[test]
fn reports_the_same_path_through_ratatui() {
    assert_reports(
        r#"fn f(out: &mut Vec<u8>) {
    execute!(out, ratatui::crossterm::terminal::EnterAlternateScreen);
}"#,
        &["EnterAlternateScreen"],
    );
}

#[test]
fn reports_a_glob_import_then_the_bare_name() {
    assert_reports(
        r#"use crossterm::terminal::*;
fn f(out: &mut Vec<u8>) {
    execute!(out, EnterAlternateScreen);
}"#,
        &["EnterAlternateScreen"],
    );
}

#[test]
fn reports_a_write_ansi_call_on_the_command() {
    assert_reports(
        r#"fn f(s: &mut String) {
    crossterm::terminal::EnterAlternateScreen.write_ansi(s);
}"#,
        &["EnterAlternateScreen"],
    );
}

#[test]
fn reports_a_termion_command_with_no_import() {
    assert_reports(
        r#"fn f(out: &mut Vec<u8>) {
    write!(out, "{}", termion::screen::ToAlternateScreen);
}"#,
        &["ToAlternateScreen"],
    );
}

#[test]
fn reports_the_termion_method_after_a_glob_import() {
    assert_reports(
        r#"use termion::screen::*;
fn f() {
    let _screen = stdout().into_alternate_screen();
}"#,
        &["into_alternate_screen"],
    );
}

#[test]
fn reports_the_termion_wrapper_type() {
    assert_reports(
        r#"fn f(out: Vec<u8>) {
    let _screen = termion::screen::AlternateScreen::from(out);
}"#,
        &["AlternateScreen"],
    );
}

#[test]
fn reports_the_termion_extension_trait_import() {
    assert_reports(
        r#"use termion::screen::IntoAlternateScreen;
fn f() {}"#,
        &["IntoAlternateScreen"],
    );
}

#[test]
fn reports_a_raw_identifier() {
    assert_reports(
        r#"fn f(out: &mut Vec<u8>) {
    execute!(out, r#EnterAlternateScreen);
}"#,
        &["EnterAlternateScreen"],
    );
}

// ---------------------------------------------------------------------------
// Positive: literals
// ---------------------------------------------------------------------------

#[test]
fn reports_a_lowercase_hex_escape() {
    assert_reports(r#"fn f() { print!("\x1b[?1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_an_uppercase_hex_escape() {
    assert_reports(r#"fn f() { print!("\x1B[?1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_a_unicode_escape() {
    assert_reports(r#"fn f() { print!("\u{1b}[?1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_a_byte_string() {
    assert_reports(
        r#"fn f() { print!("{:?}", b"\x1b[?1049h"); }"#,
        &[MODE_1049],
    );
}

#[test]
fn reports_a_c_string() {
    assert_reports(
        r#"fn f() { print!("{:?}", c"\x1b[?1049h"); }"#,
        &[MODE_1049],
    );
}

#[test]
fn reports_the_eight_bit_introducer_in_a_string() {
    assert_reports(r#"fn f() { print!("\u{9b}?1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_the_eight_bit_introducer_in_a_byte_string() {
    assert_reports(r#"fn f() { print!("{:?}", b"\x9b?1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_a_raw_esc_character_in_a_plain_string() {
    // The source is built at run time so that this file holds no real ESC.
    let source = format!("fn f() {{ print!(\"{}[?1049h\"); }}", char::from(0x1b_u8));
    assert_reports(&source, &[MODE_1049]);
}

#[test]
fn reports_a_raw_esc_character_in_a_raw_string() {
    // The source is built at run time so that this file holds no real ESC.
    let source = format!("fn f() {{ print!(r\"{}[?1049h\"); }}", char::from(0x1b_u8));
    assert_reports(&source, &[MODE_1049]);
}

#[test]
fn reports_only_the_alternate_mode_of_a_parameter_list() {
    assert_reports(r#"fn f() { print!("\x1b[?25;1049h"); }"#, &[MODE_1049]);
}

#[test]
fn reports_mode_47() {
    assert_reports(r#"fn f() { print!("\x1b[?47h"); }"#, &["CSI ?47 h"]);
}

#[test]
fn reports_mode_1047() {
    assert_reports(r#"fn f() { print!("\x1b[?1047h"); }"#, &["CSI ?1047 h"]);
}

// ---------------------------------------------------------------------------
// Negative
// ---------------------------------------------------------------------------

#[test]
fn ignores_comments_that_name_a_command() {
    assert_clean(
        r#"//! The module doc names EnterAlternateScreen.
/// The item doc names EnterAlternateScreen.
fn f() {
    // A line comment names EnterAlternateScreen.
    /* A block comment names EnterAlternateScreen. */
}"#,
    );
}

#[test]
fn ignores_a_string_that_only_names_a_command() {
    assert_clean(r#"fn f() { let _ = "EnterAlternateScreen failed"; }"#);
}

#[test]
fn ignores_the_leave_sequence() {
    assert_clean(r#"fn f() { print!("\x1b[?1049l"); }"#);
}

#[test]
fn ignores_an_integer_pattern() {
    assert_clean(
        r#"fn f(mode: u32) {
    match mode {
        1049 => {}
        _ => {}
    }
}"#,
    );
}

#[test]
fn ignores_sequences_that_are_not_an_alternate_mode() {
    assert_clean(
        r#"fn f() {
    print!("\x1b[?25h");
    print!("\x1b[1049h");
    print!("\x1b[?10490h");
}"#,
    );
}

#[test]
fn ignores_an_identifier_that_only_contains_a_listed_name() {
    assert_clean(
        r#"struct AlternateScreenGuard;
fn f(_guard: AlternateScreenGuard) {}"#,
    );
}

#[test]
fn ignores_a_doc_comment_that_writes_the_escape_as_text() {
    assert_clean(
        r#"/// Writes \x1b[?1049h to enter.
fn f() {}"#,
    );
}

// ---------------------------------------------------------------------------
// Exemptions and refusals
// ---------------------------------------------------------------------------

#[test]
fn does_not_report_an_exempt_file_and_spelling() {
    let dir = workspace_at(
        ENTRANCE_FILE,
        r#"fn f(out: &mut Vec<u8>) {
    execute!(out, crossterm::terminal::EnterAlternateScreen);
}"#,
    );
    let report = alternate_screen::audit(dir.path()).expect("a verdict");
    assert!(report.is_compliant(), "{report}");
}

#[test]
fn reports_a_new_spelling_in_an_exempt_file() {
    let dir = workspace_at(
        ENTRANCE_FILE,
        r#"fn f(out: &mut Vec<u8>) {
    write!(out, "{}", termion::screen::ToAlternateScreen);
}"#,
    );
    let report = alternate_screen::audit(dir.path()).expect("a verdict");
    let spellings: Vec<&str> = report
        .offenders()
        .iter()
        .flat_map(|offender| offender.spellings())
        .map(String::as_str)
        .collect();
    assert_eq!(spellings, ["ToAlternateScreen"]);
}

#[test]
fn reports_an_exemption_that_matches_nothing_and_spares_one_that_matched() {
    let dir = workspace_at(
        ENTRANCE_FILE,
        r#"fn f(out: &mut Vec<u8>) {
    execute!(out, crossterm::terminal::EnterAlternateScreen);
}"#,
    );
    let report = alternate_screen::audit(dir.path()).expect("a verdict");
    let unused: Vec<(&str, &str)> = report
        .unused_exemptions()
        .iter()
        .map(|exemption| (exemption.file, exemption.spelling))
        .collect();

    assert!(
        !unused.contains(&(ENTRANCE_FILE, "EnterAlternateScreen")),
        "an exemption that matched is reported as unused: {unused:?}"
    );
    assert_eq!(
        unused.len(),
        EXEMPTIONS.len() - 1,
        "every other exemption matches nothing here: {unused:?}"
    );
}

#[test]
fn refuses_a_file_that_does_not_parse() {
    let dir = workspace("fn f( {");
    let error = alternate_screen::audit(dir.path()).expect_err("a refusal");
    assert!(
        matches!(error, AlternateScreenError::ParseSource { .. }),
        "{error}"
    );
}

#[test]
fn the_report_names_the_remedy() {
    let dir = workspace(r#"fn f() { print!("\x1b[?1049h"); }"#);
    let report = alternate_screen::audit(dir.path()).expect("a verdict");
    let text = report.to_string();
    assert!(text.contains("altscreen::Session::enter"), "{text}");
    assert!(text.contains("altscreen::RatatuiSession::enter"), "{text}");
    assert!(text.contains(MODE_1049), "{text}");
}

/// The root of this repository, as an absolute path.
fn repo_root() -> std::path::PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize {}: {e}", root.display()))
}

/// The real repository enters the alternate screen only through `altscreen`,
/// apart from the exemptions that [`EXEMPTIONS`] states.
#[test]
fn this_workspace_enters_the_alternate_screen_only_through_altscreen() {
    let report = alternate_screen::audit(&repo_root()).expect("the audit reaches a verdict");

    assert!(report.files_examined() > 0, "the audit read nothing");
    assert!(report.is_compliant(), "{report}");
    assert!(
        report.unused_exemptions().is_empty(),
        "an exemption that matches nothing has outlived the code it excused: {:?}",
        report.unused_exemptions()
    );
}
