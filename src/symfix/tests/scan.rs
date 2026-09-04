//! Tests of the walk and of the way it tells a broken link from a working one.
//!
//! Every test builds its own tree inside its own [`TempDir`] and calls
//! [`symfix::run`] with two `Vec<u8>` writers, so no test starts a process and
//! no test names a path it did not just make.
//!
//! **This tool deletes and recreates symbolic links.** A test that named a path
//! in the repository, in the home directory, or anywhere else outside its own
//! temporary directory would let the tool rewrite that path. No test in this
//! file names such a path, and every new test must keep it that way.
//!
//! `walkdir` promises no order, so a test over a tree with more than one link
//! asks whether a line is present rather than comparing the whole report
//! against one string. A test over a tree with one link compares the whole
//! report, because there the whole report is fixed.
//!
//! Several tests below build a link the operating system refuses to resolve,
//! and each one asserts that the link is an error and not a broken link. That
//! distinction is the point of the tool. A loop of links gives `ELOOP` and a
//! target behind a closed directory gives `EACCES`; neither answer says the
//! target is absent, and only an absent target is a broken link. The tool
//! reports what it cannot resolve and then leaves it alone, because a rewrite
//! of a link whose failure the tool does not understand would destroy a working
//! link to punish a directory that denied it a read.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error: on the temporary directory the test just made, on the links and files it just wrote there, and on the report it just read back. The error paths of the library are read through the writers and the summary, never through a panic"
)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use symfix::{Options, Summary};
use tempfile::TempDir;

/// The options every test starts from: this root, and nothing else asked for.
///
/// Later slices give the tool more to do, and this one function is where the
/// tests say so.
fn options(root: &Path) -> Options {
    Options {
        root: root.to_path_buf(),
        prepend: None,
        remove: None,
        dry_run: false,
        verbose: false,
        skip: Vec::new(),
    }
}

/// Everything one run of the tool produced.
struct Run {
    /// The report, which the tool wrote to the output writer.
    out: String,
    /// The diagnostics, which the tool wrote to the error writer.
    err: String,
    /// The counts the tool gave back.
    summary: Summary,
}

/// Runs the tool over `options` and collects both streams and the summary.
///
/// The tool renders a path with `Path::display`, which is lossy, so both
/// streams are text even when a tree holds a name that is not UTF-8.
fn run_with(options: &Options) -> Run {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let summary = symfix::run(options, &mut out, &mut err);
    Run {
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
        summary,
    }
}

/// Runs the tool over `root` with every other option left empty.
fn run_in(root: &Path) -> Run {
    run_with(&options(root))
}

/// Makes a symbolic link at `dir/name` that points at `target`, and gives back
/// the path of the link.
fn link_at(dir: &Path, name: &str, target: impl AsRef<Path>) -> PathBuf {
    let link = dir.join(name);
    symlink(target.as_ref(), &link).unwrap();
    link
}

/// The line the tool writes for one broken link.
fn broken_line(link: &Path, target: &str) -> String {
    format!("Broken symlink: {} -> {target}\n", link.display())
}

/// The start of the line the tool writes for one link it cannot resolve.
///
/// The rest of that line is the message the operating system gave, and the
/// wording of such a message belongs to the operating system. So a test pins
/// the path and the target, which the tool chose, and leaves the message alone.
fn unresolvable_prefix(link: &Path, target: &str) -> String {
    format!("Error: cannot resolve {} -> {target}: ", link.display())
}

#[test]
fn a_tree_with_no_symlinks_reports_none() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("plain.txt"), b"contents").unwrap();
    fs::create_dir(dir.path().join("child")).unwrap();
    fs::write(dir.path().join("child").join("deeper.txt"), b"contents").unwrap();

    let run = run_in(dir.path());

    assert_eq!(run.out, "No broken symlinks found.\n");
    assert_eq!(
        run.err,
        format!("Scanning for broken symlinks: {}\n", dir.path().display())
    );
    assert_eq!(run.summary.broken, 0);
}

#[test]
fn a_broken_symlink_is_reported_and_counted() {
    let dir = TempDir::new().unwrap();
    let link = link_at(dir.path(), "link", "missing.txt");

    let run = run_in(dir.path());

    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n",
            broken_line(&link, "missing.txt")
        )
    );
    assert_eq!(run.summary.broken, 1);
}

#[test]
fn a_working_link_to_a_file_is_left_alone() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("real.txt"), b"contents").unwrap();
    let link = link_at(dir.path(), "link", "real.txt");

    let run = run_in(dir.path());

    assert_eq!(run.out, "No broken symlinks found.\n");
    assert_eq!(run.summary.broken, 0);
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("real.txt"));
}

#[test]
fn a_working_link_to_a_directory_is_left_alone() {
    // A link to a directory is the case a walk gets wrong when it asks whether
    // an entry is a directory before it asks whether the entry is a link.
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("real-dir")).unwrap();
    fs::write(dir.path().join("real-dir").join("inside.txt"), b"contents").unwrap();
    let link = link_at(dir.path(), "link", "real-dir");

    let run = run_in(dir.path());

    assert_eq!(run.out, "No broken symlinks found.\n");
    assert_eq!(run.summary.broken, 0);
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("real-dir"));
}

#[test]
fn a_broken_symlink_three_directories_deep_is_found() {
    let dir = TempDir::new().unwrap();
    let deep = dir.path().join("a").join("b").join("c");
    fs::create_dir_all(&deep).unwrap();
    let link = link_at(&deep, "link", "missing.txt");

    let run = run_in(dir.path());

    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n",
            broken_line(&link, "missing.txt")
        )
    );
    assert_eq!(run.summary.broken, 1);
}

#[test]
fn a_broken_symlink_with_a_multibyte_target_keeps_the_target_whole() {
    let dir = TempDir::new().unwrap();
    let target = "日本語/café/🎉.txt";
    let link = link_at(dir.path(), "link", target);

    let run = run_in(dir.path());

    assert_eq!(
        run.out,
        format!("{}Found 1 broken symlink(s).\n", broken_line(&link, target))
    );
    assert_eq!(run.summary.broken, 1);
}

#[test]
fn a_broken_symlink_whose_target_is_not_utf8_is_reported() {
    let dir = TempDir::new().unwrap();
    // The last byte begins no UTF-8 sequence, so this target is a name the
    // kernel accepts and a Rust string cannot hold. The tool renders it
    // lossily, thus the test pins the shape of the line and the count, and not
    // the replacement characters the renderer chose.
    let target = OsString::from_vec(vec![0x66, 0x6f, 0x6f, 0x80]);
    link_at(dir.path(), "link", PathBuf::from(target));

    let run = run_in(dir.path());

    assert!(
        run.out.starts_with("Broken symlink: "),
        "the report begins with the broken link: {:?}",
        run.out
    );
    assert!(
        run.out.ends_with("Found 1 broken symlink(s).\n"),
        "the report ends with the count: {:?}",
        run.out
    );
    assert_eq!(run.summary.broken, 1);
}

#[test]
fn two_broken_symlinks_and_one_working_link_give_a_count_of_two() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("real.txt"), b"contents").unwrap();
    link_at(dir.path(), "works", "real.txt");
    let first = link_at(dir.path(), "first", "missing-one.txt");
    let second = link_at(dir.path(), "second", "missing-two.txt");

    let run = run_in(dir.path());

    // `walkdir` gives no order, so the two broken lines can arrive either way
    // around. The test asks whether each line is present.
    assert!(
        run.out.contains(&broken_line(&first, "missing-one.txt")),
        "the first broken link is reported: {:?}",
        run.out
    );
    assert!(
        run.out.contains(&broken_line(&second, "missing-two.txt")),
        "the second broken link is reported: {:?}",
        run.out
    );
    assert!(
        run.out.ends_with("Found 2 broken symlink(s).\n"),
        "the report ends with the count: {:?}",
        run.out
    );
    assert_eq!(run.summary.broken, 2);
}

#[test]
fn verbose_names_every_symlink_the_walk_finds() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("real.txt"), b"contents").unwrap();
    let works = link_at(dir.path(), "works", "real.txt");
    let broken = link_at(dir.path(), "broken", "missing.txt");

    let run = run_with(&Options {
        verbose: true,
        ..options(dir.path())
    });

    // A working link is named on the error stream and stays out of the report.
    assert!(
        run.err
            .contains(&format!("Found symlink: {}\n", works.display())),
        "the working link is named: {:?}",
        run.err
    );
    assert!(
        run.err
            .contains(&format!("Found symlink: {}\n", broken.display())),
        "the broken link is named: {:?}",
        run.err
    );
    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n",
            broken_line(&broken, "missing.txt")
        )
    );
}

#[test]
fn a_directory_the_walk_cannot_read_gives_a_warning_and_the_walk_goes_on() {
    let dir = TempDir::new().unwrap();
    let link = link_at(dir.path(), "link", "missing.txt");
    let closed = dir.path().join("closed");
    fs::create_dir(&closed).unwrap();
    fs::write(closed.join("inside.txt"), b"contents").unwrap();
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
    // A process that may read a directory whose mode is 000 — a process running
    // as root — gets no warning at all, so the warning is only asserted when
    // the mode took effect.
    let still_readable = fs::read_dir(&closed).is_ok();

    let run = run_in(dir.path());

    // The mode goes back before any assertion, else a failing assertion would
    // leave a directory the temporary directory cannot delete.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o700)).unwrap();

    if !still_readable {
        assert!(
            run.err
                .contains(&format!("Warning: cannot read {}", closed.display())),
            "the unreadable directory is named: {:?}",
            run.err
        );
    }
    // The walk carried on: the link beside the unreadable directory is still
    // reported and still counted.
    assert!(
        run.out.contains(&broken_line(&link, "missing.txt")),
        "the link beside the unreadable directory is reported: {:?}",
        run.out
    );
    assert_eq!(run.summary.broken, 1);
}

#[test]
fn two_links_that_point_at_each_other_are_errors_and_not_broken() {
    // Each link resolves to the other, so the operating system gives `ELOOP`.
    // That answer is not "the target is absent", thus neither link is broken
    // and neither one may be rewritten.
    let dir = TempDir::new().unwrap();
    let first = link_at(dir.path(), "a", "b");
    let second = link_at(dir.path(), "b", "a");

    let run = run_in(dir.path());

    assert_eq!(run.summary.broken, 0);
    assert_eq!(run.summary.errors, 2);
    assert!(
        !run.out.contains("Broken symlink:"),
        "no link is called broken: {:?}",
        run.out
    );
    assert_eq!(run.out, "No broken symlinks found.\n");
    assert!(
        run.err.contains(&unresolvable_prefix(&first, "b")),
        "the first link is named on the error stream: {:?}",
        run.err
    );
    assert!(
        run.err.contains(&unresolvable_prefix(&second, "a")),
        "the second link is named on the error stream: {:?}",
        run.err
    );

    // Nothing was rewritten: both links are still links, and both still hold
    // the target they were made with.
    assert!(
        fs::symlink_metadata(&first)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the first link is still a link"
    );
    assert!(
        fs::symlink_metadata(&second)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the second link is still a link"
    );
    assert_eq!(fs::read_link(&first).unwrap(), Path::new("b"));
    assert_eq!(fs::read_link(&second).unwrap(), Path::new("a"));
}

#[test]
fn a_link_that_points_at_itself_is_an_error_and_not_broken() {
    // A link that names itself is the shortest loop there is, and it gives the
    // same `ELOOP` a longer loop gives.
    let dir = TempDir::new().unwrap();
    let link = link_at(dir.path(), "self", "self");

    let run = run_in(dir.path());

    assert_eq!(run.summary.broken, 0);
    assert_eq!(run.summary.errors, 1);
    assert_eq!(run.out, "No broken symlinks found.\n");
    assert!(
        run.err.contains(&unresolvable_prefix(&link, "self")),
        "the link is named on the error stream: {:?}",
        run.err
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("self"));
}

#[test]
fn a_link_whose_target_sits_behind_a_closed_directory_is_an_error_and_not_broken() {
    // This is the case the Go tool this port replaces gets wrong. Its `os.Stat`
    // gives `EACCES`, it reads every error as absence, and it then rewrites a
    // link whose target is there and is only out of reach.
    let dir = TempDir::new().unwrap();
    let closed = dir.path().join("closed");
    fs::create_dir(&closed).unwrap();
    fs::write(closed.join("target.txt"), b"contents").unwrap();
    let link = link_at(dir.path(), "link", "closed/target.txt");
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o000)).unwrap();
    // A process that may read a directory whose mode is 000 — a process running
    // as root — resolves the link and finds nothing to report at all, so the
    // assertions only run when the mode took effect.
    let still_readable = fs::read_dir(&closed).is_ok();

    let run = run_in(dir.path());

    // The mode goes back before any assertion, else a failing assertion would
    // leave a directory the temporary directory cannot delete.
    fs::set_permissions(&closed, fs::Permissions::from_mode(0o700)).unwrap();

    if !still_readable {
        assert_eq!(run.summary.broken, 0);
        assert_eq!(run.summary.errors, 1);
        assert!(
            run.err
                .contains(&unresolvable_prefix(&link, "closed/target.txt")),
            "the link is named on the error stream: {:?}",
            run.err
        );
        assert!(
            !run.out.contains("Broken symlink:"),
            "the link is not called broken: {:?}",
            run.out
        );
        assert_eq!(
            fs::read_link(&link).unwrap(),
            Path::new("closed/target.txt")
        );
    }
}

#[test]
fn a_broken_link_and_an_unresolvable_link_are_counted_apart() {
    let dir = TempDir::new().unwrap();
    let broken = link_at(dir.path(), "broken", "missing.txt");
    let looped = link_at(dir.path(), "looped", "looped");

    let run = run_in(dir.path());

    assert_eq!(run.summary.broken, 1);
    assert_eq!(run.summary.errors, 1);
    // The whole report is fixed here, because only the broken link writes to
    // it. The link that cannot be resolved is a diagnostic, and a diagnostic
    // goes to the other stream.
    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n",
            broken_line(&broken, "missing.txt")
        )
    );
    assert!(
        run.err.contains(&unresolvable_prefix(&looped, "looped")),
        "the link that cannot be resolved is named on the error stream: {:?}",
        run.err
    );
}
