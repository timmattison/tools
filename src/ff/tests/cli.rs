//! Black-box tests of the `ff` binary, through its real command line: the
//! output, the summary on standard error, and the exit status.

#[path = "common/tree.rs"]
mod tree;

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use tree::{sorted, Tree};

/// A command for the built `ff`, with no inherited git environment.
fn ff(args: &[&str]) -> std::process::Command {
    let mut command = childenv::fixture_command(env!("CARGO_BIN_EXE_ff"));
    command.args(args);
    command
}

/// Runs `ff` to the end, with each path of `paths` after `args`.
fn run(args: &[&str], paths: &[&Path]) -> Output {
    ff(args).args(paths).output().expect("run ff")
}

fn stdout_lines(output: &Output) -> Vec<PathBuf> {
    sorted(
        String::from_utf8(output.stdout.clone())
            .expect("the output is UTF-8")
            .lines()
            .map(PathBuf::from)
            .collect(),
    )
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("the summary is UTF-8")
}

#[test]
fn ff_prints_each_match_and_a_summary_and_exits_zero() {
    let tree = Tree::new("cli-match");
    let expected = vec![tree.file("120.mp4"), tree.file("deep/er/120.MP4")];
    tree.file("deep/other.mp4");

    let output = run(&["120.mp4"], &[tree.root()]);

    assert_eq!(stdout_lines(&output), sorted(expected));
    let summary = stderr(&output);
    assert!(summary.contains("ff: matches: 2\n"), "{summary}");
    assert!(
        summary.contains("ff: incomplete subtrees: 0\n"),
        "{summary}"
    );
    assert_eq!(output.status.code(), Some(0), "{summary}");
}

#[test]
fn print0_ends_each_match_with_a_nul() {
    let tree = Tree::new("cli-print0");
    let first = tree.file("a/120.mp4");
    let second = tree.file("b/120.mp4");

    let output = run(&["-0", "120.mp4"], &[tree.root()]);

    let mut printed: Vec<&[u8]> = output.stdout.split(|&byte| byte == 0).collect();
    assert_eq!(printed.pop(), Some(&b""[..]), "the output ends with a NUL");
    printed.sort_unstable();
    let mut expected = [first, second].map(|path| path.into_os_string().into_encoded_bytes());
    expected.sort_unstable();
    assert_eq!(
        printed,
        expected.iter().map(Vec::as_slice).collect::<Vec<_>>()
    );
}

#[test]
fn case_sensitive_matches_only_the_same_case() {
    let tree = Tree::new("cli-case");
    let lower = tree.file("120.mp4");
    tree.file("x/120.MP4");

    let output = run(&["-s", "120.mp4"], &[tree.root()]);

    assert_eq!(stdout_lines(&output), vec![lower]);
}

#[test]
fn regex_takes_a_regular_expression() {
    let tree = Tree::new("cli-regex");
    let expected = vec![tree.file("120.mp4"), tree.file("x/150.MP4")];
    tree.file("x/1200.mp4");

    let output = run(&["-e", r"^1\d0\.mp4$"], &[tree.root()]);

    assert_eq!(stdout_lines(&output), sorted(expected));
}

#[test]
fn a_path_that_does_not_exist_is_an_incomplete_subtree_and_exits_one() {
    let tree = Tree::new("cli-missing");
    let missing = tree.root().join("missing");

    let output = run(&["--show-skipped", "120.mp4"], &[&missing]);

    let summary = stderr(&output);
    assert!(
        summary.contains("ff: incomplete subtrees: 1\n"),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("ff: incomplete: {}: ", missing.display())),
        "{summary}"
    );
    assert_eq!(output.status.code(), Some(1), "{summary}");
}

#[test]
fn a_denied_directory_is_counted_and_named_only_with_show_skipped() {
    // SAFETY: `geteuid` reads the effective user id, and it cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("root opens a directory with mode 000, thus this test proves nothing as root");
        return;
    }
    let tree = Tree::new("cli-denied");
    tree.file("locked/120.mp4");
    let locked = tree.root().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("lock the directory");

    let quiet = run(&["120.mp4"], &[tree.root()]);
    let named = run(&["--show-skipped", "120.mp4"], &[tree.root()]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("unlock the directory");

    let quiet_summary = stderr(&quiet);
    assert!(
        quiet_summary.contains("ff: denied directories: 1 "),
        "{quiet_summary}"
    );
    assert!(!quiet_summary.contains("ff: denied: "), "{quiet_summary}");
    assert!(
        quiet_summary.contains("ff: --show-skipped names each one\n"),
        "{quiet_summary}"
    );
    assert_eq!(quiet.status.code(), Some(0), "a refusal is normal");

    let named_summary = stderr(&named);
    assert!(
        named_summary.contains(&format!("ff: denied: {}\n", locked.display())),
        "{named_summary}"
    );
}

#[test]
fn a_named_path_that_the_system_refuses_exits_one() {
    // SAFETY: `geteuid` reads the effective user id, and it cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("root opens a directory with mode 000, thus this test proves nothing as root");
        return;
    }
    let tree = Tree::new("cli-refused-root");
    tree.file("locked/120.mp4");
    let locked = tree.root().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("lock the directory");

    let output = run(&["--show-skipped", "120.mp4"], &[&locked]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("unlock the directory");

    let summary = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "{summary}");
    assert!(
        summary.contains("ff: incomplete subtrees: 1\n"),
        "{summary}"
    );
    assert!(
        summary.contains(&format!("ff: incomplete: {}: ", locked.display())),
        "{summary}"
    );
    assert!(!summary.contains("ff: denied"), "{summary}");
}

#[test]
fn a_glob_with_a_slash_is_refused_with_exit_two() {
    let tree = Tree::new("cli-slash");

    let output = run(&["Users/*.mp4"], &[tree.root()]);

    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).starts_with("ff: "), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn a_closed_output_stops_the_walk_quietly() {
    let tree = Tree::new("cli-closed-pipe");
    for index in 0..3000 {
        tree.file(&format!(
            "{}/a-long-name-that-fills-the-pipe-buffer-{index}.mp4",
            index % 10
        ));
    }

    let mut child = ff(&["*.mp4"])
        .arg(tree.root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start ff");
    drop(child.stdout.take());
    let mut summary = String::new();
    child
        .stderr
        .take()
        .expect("the standard error of ff")
        .read_to_string(&mut summary)
        .expect("read the standard error of ff");
    let status = child.wait().expect("wait for ff");

    assert_eq!(status.code(), Some(0), "{summary}");
    assert_eq!(summary, "", "a reader that stops early is not an error");
}

#[test]
fn version_names_the_tool() {
    let output = ff(&["--version"]).output().expect("run ff");

    let version = String::from_utf8(output.stdout).expect("the version is UTF-8");
    assert!(version.starts_with("ff 0.1.0 ("), "{version}");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn a_search_of_the_whole_machine_is_refused_off_macos() {
    let output = ff(&["120.mp4"]).output().expect("run ff");

    assert_eq!(output.status.code(), Some(2));
    let message = stderr(&output);
    assert!(message.contains("macOS"), "{message}");
    assert!(output.stdout.is_empty());
}

#[test]
fn a_search_of_dot_inside_library_skips_the_file_provider_directories() {
    let tree = Tree::new("cli-cloud-dot");
    tree.file("Library/CloudStorage/GoogleDrive-me@example.com/120.mp4");
    tree.file("Library/Mobile Documents/com~apple~CloudDocs/120.mp4");
    tree.file("Library/Other/120.mp4");

    let output = ff(&["120.mp4", "."])
        .current_dir(tree.root().join("Library"))
        .output()
        .expect("run ff");

    assert_eq!(
        stdout_lines(&output),
        vec![PathBuf::from("./Other/120.mp4")]
    );
    let summary = stderr(&output);
    assert!(
        summary.contains("ff: cloud directories skipped: 2 "),
        "{summary}"
    );
    assert_eq!(output.status.code(), Some(0), "{summary}");
}

#[test]
fn include_cloud_searches_the_file_provider_directories() {
    let tree = Tree::new("cli-include-cloud");
    let cloud = tree.file("Library/CloudStorage/GoogleDrive-me@example.com/120.mp4");
    let other = tree.file("Library/Other/120.mp4");

    let skipped = run(&["120.mp4"], &[tree.root()]);
    let searched = run(&["--include-cloud", "120.mp4"], &[tree.root()]);

    assert_eq!(stdout_lines(&skipped), vec![other.clone()]);
    let skipped_summary = stderr(&skipped);
    assert!(
        skipped_summary.contains("ff: cloud directories skipped: 1 "),
        "{skipped_summary}"
    );
    assert_eq!(stdout_lines(&searched), sorted(vec![cloud, other]));
    let searched_summary = stderr(&searched);
    assert!(
        !searched_summary.contains("ff: cloud directories skipped"),
        "{searched_summary}"
    );
    assert_eq!(searched.status.code(), Some(0), "{searched_summary}");
}

/// A mount point cannot be made in a temporary directory without privileges.
/// So this test reads `/System/Volumes`, which holds the data volume
/// `/System/Volumes/Data` on each Mac. Each other child of it is a mount point
/// or an empty directory. With `-x`, `ff` prints nothing and exits. Without
/// `-x`, it enters the data volume and prints a match within milliseconds, so
/// the test reads one line and stops `ff`, and a regression fails fast.
#[cfg(target_os = "macos")]
#[test]
fn one_file_system_does_not_enter_the_data_volume_below_a_named_path() {
    use std::io::BufRead;

    let mut child = ff(&["-x", "Users", "/System/Volumes"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start ff");
    let stdout = child.stdout.take().expect("the output is piped");
    let mut line = String::new();
    let read = std::io::BufReader::new(stdout).read_line(&mut line);
    let _ = child.kill();
    let _ = child.wait();
    read.expect("read the first line of the output");

    assert!(
        line.is_empty(),
        "-x entered a volume below /System/Volumes: {line}"
    );
}
