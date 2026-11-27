//! Tests of the added date of a definition: the line rule, the read of the
//! history of one file, and the read of the history of each origin file
//! through git and yadm.
//!
//! The fixture tests give `read` a stub yadm by its path, or a path where no
//! program is. They never start the yadm of the person who runs the tests.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use gitscratch::testing::{DetachedGitDirRepo, TestRepo};
use wdid::added::{added_dates, defined_names, read, AddedDates, Defined, Programs};

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
        assert_eq!(defined_names(line), vec![Defined::alias(name)], "{line}");
    }
}

#[test]
fn an_alias_command_defines_each_of_its_names() {
    assert_eq!(
        defined_names("alias gs='git status' gd='git diff'"),
        vec![Defined::alias("gs"), Defined::alias("gd")]
    );
    assert_eq!(
        defined_names("alias -s md=glow txt=less"),
        vec![Defined::alias("*.md"), Defined::alias("*.txt")]
    );
    let braces = [
        ("alias -s {md,txt}=glow", vec!["*.md", "*.txt"]),
        ("alias {a1,a2}=y", vec!["a1", "a2"]),
        ("alias {b=1,c=2}", vec!["b", "c"]),
        ("alias x{p,{q,r}}=z", vec!["xp", "xq", "xr"]),
        ("alias {a,\"b c\"}=w", vec!["a", "b c"]),
        ("alias x={a,b}", vec!["x"]),
        ("alias x=a{1..3}", vec!["x"]),
        ("alias {a,b}=x{1..3}", vec!["a", "b"]),
    ];
    for (line, names) in braces {
        let expected: Vec<Defined> = names.into_iter().map(Defined::alias).collect();
        assert_eq!(defined_names(line), expected, "{line}");
    }
}

/// The cap of 1024 words bounds the work for one line of a dotfile. A word
/// past the cap gives the name of its text as written, thus a long value
/// keeps the name.
#[test]
fn an_alias_word_past_the_brace_cap_keeps_a_plain_name() {
    let line = format!("alias x={}", "{a,b}".repeat(11));
    assert_eq!(defined_names(&line), vec![Defined::alias("x")]);
}

/// Only the shell knows the name of a word that holds an expansion, a glob,
/// or a brace sequence, thus such a word defines nothing. A name with a brace
/// list that grows past the cap of 1024 words defines nothing too.
#[test]
fn an_alias_name_that_the_shell_must_expand_defines_nothing() {
    let too_long = format!("alias {}=x", "{a,b}".repeat(11));
    let lines = [
        "alias $x=y",
        "alias \"$p\"=y",
        "alias *x=y",
        "alias {1..3}=n",
        too_long.as_str(),
    ];
    for line in lines {
        assert_eq!(defined_names(line), Vec::<Defined>::new(), "{line}");
    }
}

/// A line that opens a quote and does not close it goes on to the next line.
/// The read gives the rest of the line to the value, thus the names come from
/// the words of the line as the shell reads them.
#[test]
fn an_alias_line_with_an_open_quote_reads_the_rest_of_the_line_as_its_value() {
    let cases: [(&str, &[&str]); 10] = [
        ("alias q1='open q2=z", &["q1"]),
        ("alias -s {jpg,png}='open", &["*.jpg", "*.png"]),
        ("alias a=1 b='open", &["a", "b"]),
        ("alias 'll=ls -l", &["ll"]),
        ("alias x=\"it's", &["x"]),
        ("alias x=\"$(date", &["x"]),
        ("alias {a,b}=`date`", &["a", "b"]),
        ("alias gs='git status' \\", &["gs"]),
        ("alias foo=\"cmd1 && \\", &["foo"]),
        ("alias a=1 b=\"cmd1 && \\", &["a", "b"]),
    ];
    for (line, names) in cases {
        let expected: Vec<Defined> = names.iter().map(|name| Defined::alias(*name)).collect();
        assert_eq!(defined_names(line), expected, "{line}");
    }
}

/// A word of an open value is not a name. A name that holds an expansion is
/// not a name either.
#[test]
fn an_alias_line_with_an_open_quote_gives_no_false_name() {
    let lines = [
        "alias $x='y",
        "alias $x='y z=w",
        "alias \"$x=y",
        "alias x; y=\"$(date",
    ];
    for line in lines {
        assert_eq!(defined_names(line), Vec::<Defined>::new(), "{line}");
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
        ("alias () { builtin alias \"$@\" }", "alias"),
        ("alias ( ) {", "alias"),
        ("日本() { echo 🎉 }", "日本"),
    ];
    for (line, name) in cases {
        assert_eq!(defined_names(line), vec![Defined::function(name)], "{line}");
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
        assert!(defined_names(line).is_empty(), "{line:?}");
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
    assert_eq!(
        dates.get(&Defined::function("myf")),
        Some(&date(2025, 3, 4))
    );
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

/// `%as` gives the author date, and a rebase or a cherry-pick can put a commit
/// with an older author date above a commit with a newer one. Thus the oldest
/// date must win in each order, not the first or the last date of the read.
#[test]
fn the_oldest_date_wins_in_each_order_of_the_records() {
    let older = "\u{0}2020-01-02\n\ndiff --git a/f b/f\n@@ -0,0 +1 @@\n+alias x=y\n";
    let newer = "\u{0}2024-05-06\n\ndiff --git a/f b/f\n@@ -0,0 +1 @@\n+alias x=y\n";
    for log in [format!("{older}{newer}"), format!("{newer}{older}")] {
        assert_eq!(
            added_dates(&log).get(&Defined::alias("x")),
            Some(&date(2020, 1, 2)),
            "{log:?}"
        );
    }
    let file = Path::new("/home/me/.zshrc");
    for order in [
        [date(2020, 1, 2), date(2024, 5, 6)],
        [date(2024, 5, 6), date(2020, 1, 2)],
    ] {
        let mut dates = AddedDates::default();
        for day in order {
            dates.record(file, Defined::alias("x"), day);
        }
        assert_eq!(
            dates.get(file, &Defined::alias("x")),
            Some(date(2020, 1, 2)),
            "{order:?}"
        );
    }
}

/// zsh and bash take more than one `name=value` in one `alias` command, thus
/// `alias gs='git status' gd='git diff'` defines `gd` too. Each such name gets
/// the date of the commit that adds the line. A word after `#` or after an
/// operator belongs to no alias of the line, and a value that the line leaves
/// open does not give a false name.
#[test]
fn each_name_of_an_alias_command_gets_the_date() {
    let added = [
        "alias gs='git status' gd='git diff'",
        "alias -g G='| grep' L='| less'",
        "alias -s md=glow txt=less",
        "alias shown gp='git push'",
        "alias h=\"echo $HOME\" w=z",
        "alias c1=x # c2=y",
        "alias s1=x; s2=y",
        "alias q1='open q2=z",
        "alias -s {jpg,png}=open",
        "alias {a1,a2}=y",
    ];
    let hunk: String = added.iter().map(|line| format!("+{line}\n")).collect();
    let log = format!("\u{0}2025-01-01\n\ndiff --git a/f b/f\n@@ -0,0 +1,10 @@\n{hunk}");
    let dates = added_dates(&log);
    let names: BTreeSet<Defined> = dates.keys().cloned().collect();
    let expected: BTreeSet<Defined> = [
        "gs", "gd", "G", "L", "*.md", "*.txt", "gp", "h", "w", "c1", "s1", "q1", "*.jpg", "*.png",
        "a1", "a2",
    ]
    .into_iter()
    .map(Defined::alias)
    .collect();
    assert_eq!(names, expected);
    for (defined, day) in &dates {
        assert_eq!(*day, date(2025, 1, 1), "{defined:?}");
    }
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

/// The date of the oldest commit of the fixtures.
const OLDEST: &str = "2019-03-04T12:00:00+00:00";

/// The date of the middle commit of the fixtures.
const MIDDLE: &str = "2021-06-07T12:00:00+00:00";

/// The date of the newest commit of the fixtures.
const NEWEST: &str = "2023-08-09T12:00:00+00:00";

/// Writes `contents` to `name` below `root`, and makes the directories that
/// lead there.
fn write(root: &Path, name: &str, contents: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// Commits each pair of a name and contents into `repo`, with `date` as the
/// author date.
fn commit_at(repo: &TestRepo, files: &[(&str, &str)], date: &str) {
    for (name, contents) in files {
        write(repo.path(), name, contents);
        repo.git(&["add", name]);
    }
    repo.git(&["commit", "-q", "-m", "change", "--date", date]);
}

/// Commits each pair of a name and contents into the yadm-style `repo`, with
/// `date` as the author date.
fn commit_detached_at(repo: &DetachedGitDirRepo, files: &[(&str, &str)], date: &str) {
    for (name, contents) in files {
        write(repo.work_tree(), name, contents);
        repo.git(&["add", name]);
    }
    repo.git(&["commit", "-q", "-m", "change", "--date", date]);
}

/// A directory of stub programs.
struct Stubs {
    dir: tempfile::TempDir,
}

impl Stubs {
    /// Makes an empty directory of stubs.
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// Writes a stub `yadm` that prints `git_dir` and records each call in
    /// [`Stubs::yadm_log`].
    fn yadm(&self, git_dir: &Path) -> OsString {
        let path = self.dir.path().join("yadm");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nprintf '%s\\n' {}\n",
            shellquote::shell_quote(&self.yadm_log().to_string_lossy()),
            shellquote::shell_quote(&git_dir.to_string_lossy()),
        );
        gitscratch::testing::executable::write_executable(&path, body).unwrap();
        path.into_os_string()
    }

    /// Gives a path where no program is: a yadm that is not installed.
    fn missing(&self) -> OsString {
        self.dir.path().join("no-such-program").into_os_string()
    }

    /// Writes a `git` that records the arguments of each call in
    /// [`Stubs::git_log`], then runs the git of the `PATH` of the test.
    fn recording_git(&self) -> OsString {
        let path = self.dir.path().join("git");
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexec git \"$@\"\n",
            shellquote::shell_quote(&self.git_log().to_string_lossy()),
        );
        gitscratch::testing::executable::write_executable(&path, body).unwrap();
        path.into_os_string()
    }

    /// The file where the stub `yadm` records its calls.
    fn yadm_log(&self) -> PathBuf {
        self.dir.path().join("yadm.log")
    }

    /// The file where the recording `git` records its calls.
    fn git_log(&self) -> PathBuf {
        self.dir.path().join("git.log")
    }

    /// The lines of `file`, or none when it does not exist.
    fn calls(file: &Path) -> Vec<String> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

/// The programs with the git of the `PATH` and the yadm `yadm`.
fn programs(yadm: OsString) -> Programs {
    Programs {
        git: OsString::from("git"),
        yadm,
    }
}

/// Gives the set of `files`.
fn files(files: &[&Path]) -> BTreeSet<PathBuf> {
    files.iter().map(|file| file.to_path_buf()).collect()
}

/// Says if the call `line` of the recording `git` runs `subcommand`.
fn runs(line: &str, subcommand: &str) -> bool {
    line.split(' ').any(|word| word == subcommand)
}

/// The text of a shell file with an alias and a function.
const OLD_LINES: &str = "alias old='echo old'\noldfn() { true }\n";

#[test]
fn a_file_in_a_plain_git_repository_gets_the_dates_of_its_history() {
    let repo = TestRepo::init();
    commit_at(&repo, &[("shell/aliases.zsh", OLD_LINES)], OLDEST);
    commit_at(
        &repo,
        &[(
            "shell/aliases.zsh",
            "alias old='echo old'\noldfn() { true }\nfunction newfn {\n  true\n}\n",
        )],
        NEWEST,
    );
    let file = repo.path().join("shell/aliases.zsh");
    let stubs = Stubs::new();

    let dates = read(&files(&[&file]), None, &programs(stubs.missing()));

    assert_eq!(
        dates.get(&file, &Defined::alias("old")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&file, &Defined::function("oldfn")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&file, &Defined::function("newfn")),
        Some(date(2023, 8, 9))
    );
    assert_eq!(dates.get(&file, &Defined::alias("newfn")), None);
}

#[test]
fn a_renamed_file_keeps_the_dates_from_before_the_rename() {
    let repo = TestRepo::init();
    commit_at(&repo, &[("old-name.zsh", OLD_LINES)], OLDEST);
    repo.git(&["mv", "old-name.zsh", "new-name.zsh"]);
    repo.git(&["commit", "-q", "-m", "rename", "--date", MIDDLE]);
    let file = repo.path().join("new-name.zsh");
    let stubs = Stubs::new();

    let dates = read(&files(&[&file]), None, &programs(stubs.missing()));

    assert_eq!(
        dates.get(&file, &Defined::alias("old")),
        Some(date(2019, 3, 4))
    );
}

#[test]
fn a_file_under_home_without_yadm_gets_the_dates_of_its_work_tree() {
    let repo = TestRepo::init();
    commit_at(&repo, &[(".zshrc", OLD_LINES)], MIDDLE);
    let file = repo.path().join(".zshrc");
    let stubs = Stubs::new();

    let dates = read(
        &files(&[&file]),
        Some(repo.path()),
        &programs(stubs.missing()),
    );

    assert_eq!(
        dates.get(&file, &Defined::alias("old")),
        Some(date(2021, 6, 7))
    );
}

#[test]
fn a_file_of_yadm_gets_the_dates_from_the_git_directory_of_yadm() {
    let repo = DetachedGitDirRepo::nested();
    commit_detached_at(&repo, &[(".zshrc", OLD_LINES)], OLDEST);
    commit_detached_at(
        &repo,
        &[(".zsh/more.zsh", "alias -g G='| grep'\nalias -s md=glow\n")],
        NEWEST,
    );
    let zshrc = repo.work_tree().join(".zshrc");
    let more = repo.work_tree().join(".zsh/more.zsh");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(repo.git_dir());

    let dates = read(
        &files(&[&zshrc, &more]),
        Some(repo.work_tree()),
        &programs(yadm),
    );

    assert_eq!(
        dates.get(&zshrc, &Defined::alias("old")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&zshrc, &Defined::function("oldfn")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&more, &Defined::alias("G")),
        Some(date(2023, 8, 9))
    );
    assert_eq!(
        dates.get(&more, &Defined::alias("*.md")),
        Some(date(2023, 8, 9))
    );
    assert_eq!(
        Stubs::calls(&stubs.yadm_log()),
        vec!["introspect repo".to_string()],
        "wdid asks yadm for its git directory one time"
    );
}

#[test]
fn an_alternate_template_gives_the_dates_of_its_rendered_file() {
    let repo = DetachedGitDirRepo::nested();
    commit_detached_at(&repo, &[(".zshrc##template", OLD_LINES)], OLDEST);
    repo.git(&["mv", ".zshrc##template", ".zshrc##template.default"]);
    repo.git(&["commit", "-q", "-m", "rename", "--date", MIDDLE]);
    commit_detached_at(
        &repo,
        &[(
            ".zshrc##template.default",
            "{% if yadm.os == \"Darwin\" %}\nalias old='echo old'\n{% endif %}\n\
             oldfn() { true }\nnewfn() { true }\n",
        )],
        NEWEST,
    );
    write(
        repo.work_tree(),
        ".zshrc",
        "# rendered by yadm\n\nalias old='echo old'\noldfn() { true }\nnewfn() { true }\n",
    );
    let file = repo.work_tree().join(".zshrc");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(repo.git_dir());

    let dates = read(&files(&[&file]), Some(repo.work_tree()), &programs(yadm));

    assert_eq!(
        dates.get(&file, &Defined::alias("old")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&file, &Defined::function("oldfn")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&file, &Defined::function("newfn")),
        Some(date(2023, 8, 9))
    );
}

#[test]
fn each_tracked_alternate_counts_and_the_oldest_date_wins() {
    let repo = DetachedGitDirRepo::nested();
    commit_detached_at(&repo, &[(".zshrc##os.Linux", "alias both=1\n")], OLDEST);
    commit_detached_at(
        &repo,
        &[(".zshrc##os.Darwin", "alias both=1\nalias mac=1\n")],
        NEWEST,
    );
    write(repo.work_tree(), ".zshrc", "alias both=1\nalias mac=1\n");
    let file = repo.work_tree().join(".zshrc");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(repo.git_dir());

    let dates = read(&files(&[&file]), Some(repo.work_tree()), &programs(yadm));

    assert_eq!(
        dates.get(&file, &Defined::alias("both")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&file, &Defined::alias("mac")),
        Some(date(2023, 8, 9))
    );
}

#[test]
fn a_file_that_no_repository_tracks_has_no_dates() {
    let repo = DetachedGitDirRepo::nested();
    commit_detached_at(&repo, &[(".zshrc", OLD_LINES)], OLDEST);
    write(repo.work_tree(), ".zprofile", OLD_LINES);
    let untracked_in_yadm = repo.work_tree().join(".zprofile");
    let plain = TestRepo::init();
    commit_at(&plain, &[("tracked.zsh", "alias tracked=1\n")], OLDEST);
    write(plain.path(), "untracked.zsh", OLD_LINES);
    let untracked_in_git = plain.path().join("untracked.zsh");
    let outside = gitscratch::testing::not_a_repository();
    write(outside.path(), "loose.zsh", OLD_LINES);
    let loose = outside.path().join("loose.zsh");
    let gone = outside.path().join("no-such-directory/gone.zsh");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(repo.git_dir());

    let dates = read(
        &files(&[&untracked_in_yadm, &untracked_in_git, &loose, &gone]),
        Some(repo.work_tree()),
        &programs(yadm),
    );

    for file in [&untracked_in_yadm, &untracked_in_git, &loose, &gone] {
        assert_eq!(
            dates.get(file, &Defined::alias("old")),
            None,
            "{}",
            file.display()
        );
    }
    assert_eq!(dates, AddedDates::default());
}

#[test]
fn a_yadm_that_fails_is_skipped() {
    let repo = TestRepo::init();
    commit_at(&repo, &[(".zshrc", OLD_LINES)], MIDDLE);
    let file = repo.path().join(".zshrc");
    let stubs = Stubs::new();
    let failing = stubs.dir.path().join("failing-yadm");
    gitscratch::testing::executable::write_executable(&failing, "#!/bin/sh\nexit 1\n").unwrap();

    let dates = read(
        &files(&[&file]),
        Some(repo.path()),
        &programs(failing.into_os_string()),
    );

    assert_eq!(
        dates.get(&file, &Defined::alias("old")),
        Some(date(2021, 6, 7))
    );
}

#[test]
fn one_git_log_runs_for_each_origin_file_and_not_for_each_definition() {
    let repo = TestRepo::init();
    commit_at(
        &repo,
        &[
            (
                "a.zsh",
                "alias a1=1\nalias a2=2\nalias a3=3\nfa() { true }\nfb() { true }\n",
            ),
            ("b.zsh", "alias b1=1\nalias b2=2\n"),
        ],
        OLDEST,
    );
    let a = repo.path().join("a.zsh");
    let b = repo.path().join("b.zsh");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(Path::new("/nonexistent"));
    let git = stubs.recording_git();

    let dates = read(&files(&[&a, &b]), None, &Programs { git, yadm });

    assert_eq!(dates.get(&a, &Defined::alias("a3")), Some(date(2019, 3, 4)));
    assert_eq!(dates.get(&b, &Defined::alias("b2")), Some(date(2019, 3, 4)));
    let calls = Stubs::calls(&stubs.git_log());
    assert_eq!(calls.len(), 2, "{calls:#?}");
    assert!(calls.iter().all(|line| runs(line, "log")), "{calls:#?}");
    assert!(
        Stubs::calls(&stubs.yadm_log()).is_empty(),
        "no file is under a home, thus yadm does not start"
    );
}

#[test]
fn yadm_adds_one_list_of_its_files_and_then_one_git_log_for_each_origin_file() {
    let repo = DetachedGitDirRepo::nested();
    commit_detached_at(
        &repo,
        &[
            (".zshrc", "alias a1=1\nalias a2=2\nfa() { true }\n"),
            (".zsh/more.zsh", "alias b1=1\nalias b2=2\n"),
        ],
        OLDEST,
    );
    let zshrc = repo.work_tree().join(".zshrc");
    let more = repo.work_tree().join(".zsh/more.zsh");
    let stubs = Stubs::new();
    let yadm = stubs.yadm(repo.git_dir());
    let git = stubs.recording_git();

    let dates = read(
        &files(&[&zshrc, &more]),
        Some(repo.work_tree()),
        &Programs { git, yadm },
    );

    assert_eq!(
        dates.get(&zshrc, &Defined::function("fa")),
        Some(date(2019, 3, 4))
    );
    assert_eq!(
        dates.get(&more, &Defined::alias("b1")),
        Some(date(2019, 3, 4))
    );
    let calls = Stubs::calls(&stubs.git_log());
    assert_eq!(calls.len(), 3, "{calls:#?}");
    assert_eq!(
        calls.iter().filter(|line| runs(line, "ls-files")).count(),
        1,
        "{calls:#?}"
    );
    assert_eq!(
        calls.iter().filter(|line| runs(line, "log")).count(),
        2,
        "{calls:#?}"
    );
}

#[test]
fn a_symbolic_link_to_a_file_in_a_work_tree_gets_the_dates_of_its_target() {
    let repo = TestRepo::init();
    commit_at(&repo, &[("dotfiles/zshrc", OLD_LINES)], MIDDLE);
    let home = gitscratch::testing::not_a_repository();
    let link = home.path().join(".zshrc");
    std::os::unix::fs::symlink(repo.path().join("dotfiles/zshrc"), &link).unwrap();
    let stubs = Stubs::new();

    let dates = read(
        &files(&[&link]),
        Some(home.path()),
        &programs(stubs.missing()),
    );

    assert_eq!(
        dates.get(&link, &Defined::alias("old")),
        Some(date(2021, 6, 7))
    );
}

/// The stub `yadm` of the test of a name that is not UTF-8. It prints a git
/// directory whose name ends in the byte 0xFF, written as an octal escape.
const NON_UTF8_YADM: &str = r#"#!/bin/sh
printf '/no/such/yadm-\377.git\n'
"#;

/// The stub `git` of the test of a name that is not UTF-8. It compares bytes,
/// and it answers only to the git directory and the alternate that hold the
/// byte 0xFF.
const NON_UTF8_GIT: &str = r#"#!/bin/sh
export LC_ALL=C
want=$(printf '/no/such/yadm-\377.git')
[ "$1" = --git-dir ] && [ "$2" = "$want" ] || exit 1
alternate=$(printf '.zshrc##os.\377')
mode=
last=
for argument in "$@"; do
  [ "$argument" = ls-files ] && mode=list
  [ "$argument" = log ] && mode=log
  last=$argument
done
if [ "$mode" = list ]; then
  printf '.zshrc##os.\377\000'
  exit 0
fi
if [ "$mode" = log ] && [ "$last" = "$alternate" ]; then
  printf '\0002024-02-03\n\ndiff --git a/x b/x\n@@ -0,0 +1 @@\n+alias bytes=1\n'
  exit 0
fi
exit 1
"#;

/// On Linux a file name can hold a byte that is not UTF-8. APFS refuses such a
/// name, thus the stubs print the bytes and the test makes no such file. The
/// read keeps the bytes of the git directory of yadm and of its file list, so
/// the alternate still gets its dates.
#[test]
fn a_yadm_path_that_is_not_utf8_keeps_its_bytes() {
    let home = tempfile::tempdir().unwrap();
    let stubs = Stubs::new();
    let yadm = stubs.dir.path().join("yadm");
    let git = stubs.dir.path().join("git");
    gitscratch::testing::executable::write_executable(&yadm, NON_UTF8_YADM).unwrap();
    gitscratch::testing::executable::write_executable(&git, NON_UTF8_GIT).unwrap();
    let file = home.path().join(".zshrc");

    let dates = read(
        &files(&[&file]),
        Some(home.path()),
        &Programs {
            git: git.into_os_string(),
            yadm: yadm.into_os_string(),
        },
    );

    assert_eq!(
        dates.get(&file, &Defined::alias("bytes")),
        Some(date(2024, 2, 3))
    );
}
