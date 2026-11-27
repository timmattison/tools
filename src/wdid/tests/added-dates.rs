//! Tests of the added date of a definition: the line rule, and the read of the
//! history of one file.

use chrono::NaiveDate;
use wdid::added::{added_dates, defined_name, Defined};

/// Makes a date.
fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

#[test]
fn an_alias_line_defines_its_name() {
    let cases = [
        ("alias wt='cwt'", "wt"),
        ("  alias wtf=wt -f", "wtf"),
        ("\talias gs='git status'  # a comment", "gs"),
        ("alias -g G='| grep'", "G"),
        ("alias -s md=glow", "*.md"),
        ("alias -- -='cd -'", "-"),
        ("alias 'll=ls -l'", "ll"),
        ("alias \"la=ls -a\"", "la"),
        ("alias ..='cd ..'", ".."),
        ("alias x='a=b'", "x"),
        ("alias café='echo é'", "café"),
    ];
    for (line, name) in cases {
        assert_eq!(defined_name(line), Some(Defined::alias(name)), "{line}");
    }
}

#[test]
fn a_function_line_defines_its_name() {
    let cases = [
        ("myf() { helper \"$@\" }", "myf"),
        ("myf () {", "myf"),
        ("mkcd(){", "mkcd"),
        ("  myf ( ) {", "myf"),
        ("function myf {", "myf"),
        ("function myf() {", "myf"),
        ("\tfunction myf", "myf"),
        ("function git-up\r", "git-up"),
        ("alias() { builtin alias \"$@\" }", "alias"),
        ("日本() { echo 🎉 }", "日本"),
    ];
    for (line, name) in cases {
        assert_eq!(defined_name(line), Some(Defined::function(name)), "{line}");
    }
}

#[test]
fn other_lines_define_no_name() {
    let lines = [
        "",
        "   ",
        "# alias x=y",
        "#myf() {",
        "echo alias x=y",
        "alias",
        "alias -L",
        "alias x",
        "alias =y",
        "x=y",
        "export PATH=$HOME/bin:$PATH",
        "myf(x)",
        "if (( x )); then",
        "[[ -f x ]] && source x",
        "function",
        "function {",
        "aliases=1",
        "functions[x]=y",
    ];
    for line in lines {
        assert_eq!(defined_name(line), None, "{line:?}");
    }
}

/// A history of one file, newest commit first, as the `git log` of the read
/// gives it. The alias `wt` is in the oldest commit, it is removed, and a
/// later commit adds it again. The content line `+++ b/foo` shows as
/// `++++ b/foo` in its hunk.
const LOG: &str = "\u{0}2025-03-04\n\
\n\
diff --git a/.zshrc b/.zshrc\n\
index 1111111..2222222 100644\n\
--- a/.zshrc\n\
+++ b/.zshrc\n\
@@ -0,0 +1,3 @@\n\
+alias wt='cwt'\n\
+myf() { echo }\n\
++++ b/foo\n\
\u{0}2024-05-06\n\
\n\
diff --git a/.zshrc b/.zshrc\n\
index 0000000..1111111 100644\n\
--- a/.zshrc\n\
+++ b/.zshrc\n\
@@ -1 +0,0 @@\n\
-alias wt='cwt'\n\
@@ -2,0 +2 @@ alias gone=x\n\
+alias -s md=glow\n\
\u{0}2020-01-02\n\
\n\
diff --git a/.zshrc b/.zshrc\n\
new file mode 100644\n\
index 0000000..0000000\n\
--- /dev/null\n\
+++ b/.zshrc\n\
@@ -0,0 +1,2 @@\n\
+alias wt='cwt'\n\
+alias gone=x\n";

#[test]
fn the_oldest_commit_that_adds_a_name_gives_its_date() {
    let dates = added_dates(LOG);
    assert_eq!(dates.get(&Defined::alias("wt")), Some(&date(2020, 1, 2)));
    assert_eq!(dates.get(&Defined::alias("gone")), Some(&date(2020, 1, 2)));
    assert_eq!(dates.get(&Defined::alias("*.md")), Some(&date(2024, 5, 6)));
    assert_eq!(dates.get(&Defined::function("myf")), Some(&date(2025, 3, 4)));
}

#[test]
fn a_removed_line_and_a_header_define_nothing() {
    let dates = added_dates(LOG);
    assert_eq!(
        dates.keys().cloned().collect::<Vec<_>>(),
        vec![
            Defined::alias("*.md"),
            Defined::alias("gone"),
            Defined::alias("wt"),
            Defined::function("myf"),
        ],
        "{dates:?}"
    );
}

#[test]
fn an_alias_and_a_function_of_one_name_have_their_own_dates() {
    let log = "\u{0}2024-01-01\n\ndiff --git a/f b/f\n@@ -0,0 +1 @@\n+x() { true }\n\
               \u{0}2021-01-01\n\ndiff --git a/f b/f\n@@ -0,0 +1 @@\n+alias x=y\n";
    let dates = added_dates(log);
    assert_eq!(dates.get(&Defined::alias("x")), Some(&date(2021, 1, 1)));
    assert_eq!(dates.get(&Defined::function("x")), Some(&date(2024, 1, 1)));
}

#[test]
fn a_record_without_a_date_and_a_rename_add_nothing() {
    let log = "\u{0}not a date\n\ndiff --git a/f b/f\n@@ -0,0 +1 @@\n+alias x=y\n\
               \u{0}2025-01-01\n\ndiff --git a/old b/f\nsimilarity index 100%\n\
               rename from old\nrename to f\n";
    assert!(added_dates(log).is_empty());
    assert!(added_dates("").is_empty());
}

#[test]
fn only_a_line_inside_a_hunk_is_an_added_line() {
    let log = "\u{0}2025-01-01\n\ndiff --git a/f b/f\n+alias header=1\n\
               @@ -0,0 +1 @@\n+alias real=1\n\
               diff --git a/g b/g\n+alias between=1\n";
    let dates = added_dates(log);
    assert_eq!(
        dates.keys().cloned().collect::<Vec<_>>(),
        vec![Defined::alias("real")],
        "{dates:?}"
    );
}
