//! Tests of the repair: which new target the tool builds, whether it accepts
//! that target, and what the run says about it afterwards.
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
//! Several of the tests below build a candidate target that begins with a
//! slash. That is the case the Go tool this port replaces gets wrong, and it
//! is the reason this file exists. `filepath.Join` appends an absolute second
//! argument to the first, while the operating system resolves an absolute link
//! target against the root and ignores the directory the link sits in. So the
//! Go tool can check one file, write the name of another into the link, and
//! report a repair it did not make. Each strategy carries a pair of tests that
//! pins both halves of the rule: a candidate the link would not resolve to is
//! refused however plausible it looks from the directory of the link, and an
//! absolute candidate that does resolve is accepted, because it is absence and
//! not shape that a repair answers.

#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion, not an unhandled error: on the temporary directory the test just made, on the links and files it just wrote there, and on the report it just read back. The error paths of the library are read through the writers and the summary, never through a panic"
)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use symfix::{Options, Summary};
use tempfile::TempDir;

/// The options every test starts from: this root, and nothing else asked for.
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

/// The options of a run that repairs by putting `prefix` in front of a target.
fn with_prepend(root: &Path, prefix: impl Into<OsString>) -> Options {
    Options {
        prepend: Some(prefix.into()),
        ..options(root)
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

/// Makes the directory at `dir` and every directory above it, and gives it back.
fn dir_at(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The line the tool writes for one broken link.
fn broken_line(link: &Path, target: &str) -> String {
    format!("Broken symlink: {} -> {target}\n", link.display())
}

/// The line the tool writes for one repair the prepend made.
fn prepended_line(link: &Path, target: &str) -> String {
    format!(
        "Fixed symlink by prepending: {} -> {target}\n",
        link.display()
    )
}

/// The line the tool writes when a fix flag was given and nothing came of it.
const NOTHING_FIXED: &str = "No symlinks could be fixed with the provided options.\n";

/// Builds the tree the repair tests share: a target one directory up from a
/// link that names it without the step up.
///
/// `{root}/sub/target.txt` is a file, and `{root}/sub/deeper/link` points at
/// `target.txt`, which resolves to `{root}/sub/deeper/target.txt` and is not
/// there. A prepend of `../` repairs it.
fn moved_up_one_directory(root: &Path) -> PathBuf {
    let deeper = dir_at(root, "sub/deeper");
    fs::write(root.join("sub").join("target.txt"), b"contents").unwrap();
    link_at(&deeper, "link", "target.txt")
}

#[test]
fn a_prepend_that_resolves_repairs_the_link() {
    let dir = TempDir::new().unwrap();
    let link = moved_up_one_directory(dir.path());

    let run = run_with(&with_prepend(dir.path(), "../"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        fs::metadata(&link).is_ok(),
        "the repaired link resolves: {:?}",
        fs::read_link(&link)
    );
    assert!(
        run.out.contains(&prepended_line(&link, "../target.txt")),
        "the repair is reported: {:?}",
        run.out
    );
    assert!(
        run.out.contains("Found 1 broken symlink(s).\n"),
        "the broken link is still counted: {:?}",
        run.out
    );
    assert!(
        run.out.ends_with("Fixed 1 symlink(s).\n"),
        "the report ends with the count of repairs: {:?}",
        run.out
    );
    assert!(
        !run.err.contains("Warning: cannot replace"),
        "the replacement itself raised nothing: {:?}",
        run.err
    );
}

#[test]
fn a_repaired_link_holds_the_target_as_it_was_built() {
    // The new link holds the candidate as the tool built it. Only the check
    // resolves that candidate against the directory of the link, so a relative
    // repair stays relative and a tree that moves again still works.
    let dir = TempDir::new().unwrap();
    let link = moved_up_one_directory(dir.path());

    let run = run_with(&with_prepend(dir.path(), "../"));

    assert_eq!(run.summary.fixed, 1);
    let target = fs::read_link(&link).unwrap();
    assert_eq!(target, Path::new("../target.txt"));
    assert!(
        !target.is_absolute(),
        "the repaired target is still relative: {}",
        target.display()
    );
}

#[test]
fn a_prepend_that_does_not_resolve_makes_no_repair() {
    let dir = TempDir::new().unwrap();
    let link = link_at(dir.path(), "link", "missing.txt");

    let run = run_with(&with_prepend(dir.path(), "elsewhere/"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 0,
            errors: 0
        }
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("missing.txt"));
    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n{NOTHING_FIXED}",
            broken_line(&link, "missing.txt")
        )
    );
}

#[test]
fn a_run_with_no_fix_flag_prints_neither_closing_line() {
    // This one guards the branch that was already there: a run that was never
    // asked to repair anything says nothing about repairs. It is the only test
    // in this file that passes before the repair is written, because the line
    // it forbids is a line the tool does not yet know how to print.
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
    assert_eq!(run.summary.fixed, 0);
}

#[test]
fn an_absolute_candidate_the_link_would_not_resolve_to_is_refused() {
    // This is the defect the port exists to close, and this tree is built so
    // that the two spellings of the check disagree and the wrong one succeeds.
    //
    // `{root}/sub/link` points at `y`, and `{root}/sub/y` is not there, so the
    // link is broken. `{root}/sub/xy` is a real file. A prepend of `/x` builds
    // the candidate `/xy`.
    //
    // The Go tool checks `filepath.Join("{root}/sub", "/xy")`, and
    // `filepath.Join` appends an absolute second argument rather than replacing
    // the first. So it checks `{root}/sub/xy`, finds the file, writes `/xy`
    // into the link, prints `Fixed symlink by prepending:`, and counts a
    // repair — leaving a different broken link in place of the old one.
    //
    // The operating system resolves an absolute link target against the root,
    // so the link would resolve to `/xy` and to nothing else. `Path::join`
    // replaces the first path when the second is absolute, which is that same
    // rule, thus this tool checks `/xy`, does not find it, and refuses.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    let link = link_at(&sub, "link", "y");
    fs::write(sub.join("xy"), b"contents").unwrap();
    // The test says what it means to say only while nothing on this machine
    // answers to the name the candidate builds. Nothing is written there
    // either way: this is a read of one name, and the only path the tool may
    // write is the link inside the temporary directory.
    assert!(
        fs::symlink_metadata("/xy").is_err(),
        "this test needs /xy to be absent, and it is not"
    );

    let run = run_with(&with_prepend(dir.path(), "/x"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 0,
            errors: 0
        }
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("y"),
        "the link still holds the target it was made with"
    );
    assert!(
        !run.out.contains("Fixed"),
        "no repair is reported: {:?}",
        run.out
    );
    assert!(
        run.out.contains(NOTHING_FIXED),
        "the report says nothing could be repaired: {:?}",
        run.out
    );
}

#[test]
fn an_absolute_candidate_that_resolves_repairs_the_link() {
    // The other half of the rule. An absolute candidate is not refused as a
    // class: the tool refuses a candidate the link would not resolve to, and
    // accepts one it would, whatever shape it has.
    let dir = TempDir::new().unwrap();
    let absent = dir_at(dir.path(), "absent");
    fs::write(absent.join("target.txt"), b"contents").unwrap();
    let sub = dir_at(dir.path(), "sub");
    let link = link_at(&sub, "link", "/absent/target.txt");

    let run = run_with(&with_prepend(dir.path(), dir.path()));

    let expected = dir.path().join("absent").join("target.txt");
    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        fs::metadata(&link).is_ok(),
        "the repaired link resolves: {:?}",
        fs::read_link(&link)
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        expected,
        "the link holds the absolute candidate as it was built"
    );
    assert!(
        run.out
            .contains(&prepended_line(&link, &expected.display().to_string())),
        "the repair is reported: {:?}",
        run.out
    );
}

#[test]
fn a_repair_leaves_no_temporary_entry_behind() {
    // The replacement makes the new link under a name of its own and then
    // renames it over the old one, so that a process which dies in the middle
    // leaves either the old link or the new one and never no link at all.
    //
    // That guarantee is not visible to a black box test in one thread: the
    // atomic design and the remove-then-create design both end with the same
    // link on the disk. What is visible is the litter, so this test pins that:
    // after a repair the directory holds what the fixture made and nothing
    // else, which fails the moment the temporary link is left behind.
    let dir = TempDir::new().unwrap();
    let link = moved_up_one_directory(dir.path());
    let deeper = link.parent().unwrap();

    let run = run_with(&with_prepend(dir.path(), "../"));

    assert_eq!(run.summary.fixed, 1);
    let mut names: Vec<OsString> = fs::read_dir(deeper)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    names.sort();
    assert_eq!(names, vec![OsString::from("link")]);
}

#[test]
fn a_multibyte_target_survives_a_repair() {
    let dir = TempDir::new().unwrap();
    let nested = dir_at(dir.path(), "日本語/café");
    fs::write(nested.join("🎉.txt"), b"contents").unwrap();
    let sub = dir_at(dir.path(), "sub");
    let link = link_at(&sub, "link", "日本語/café/🎉.txt");

    let run = run_with(&with_prepend(dir.path(), "../"));

    assert_eq!(run.summary.fixed, 1);
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("../日本語/café/🎉.txt"),
        "the repaired target came back whole"
    );
    assert!(
        fs::metadata(&link).is_ok(),
        "the repaired link resolves: {:?}",
        fs::read_link(&link)
    );
    assert!(
        run.out
            .contains(&prepended_line(&link, "../日本語/café/🎉.txt")),
        "the repair is reported: {:?}",
        run.out
    );
}

#[test]
fn one_repair_among_two_broken_links_is_counted_alone() {
    let dir = TempDir::new().unwrap();
    let repairable = moved_up_one_directory(dir.path());
    let deeper = repairable.parent().unwrap().to_path_buf();
    let hopeless = link_at(&deeper, "hopeless", "nowhere.txt");

    let run = run_with(&with_prepend(dir.path(), "../"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 2,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        run.out.contains("Found 2 broken symlink(s).\n"),
        "both broken links are counted: {:?}",
        run.out
    );
    assert!(
        run.out.contains("Fixed 1 symlink(s).\n"),
        "one repair is counted: {:?}",
        run.out
    );
    assert!(
        run.out
            .contains(&prepended_line(&repairable, "../target.txt")),
        "the repair is reported: {:?}",
        run.out
    );
    assert_eq!(
        fs::read_link(&hopeless).unwrap(),
        Path::new("nowhere.txt"),
        "the link that could not be repaired was left alone"
    );
}

#[test]
fn verbose_names_the_candidate_it_tried_and_the_one_that_was_not_there() {
    // The two debug lines are the only account a user gets of a candidate the
    // tool built and then refused, so they name the candidate and not just the
    // link. The candidate that resolved gets the first line and not the second:
    // a run that said `does not exist` about a target it went on to write would
    // be worse than silence.
    let dir = TempDir::new().unwrap();
    let link = moved_up_one_directory(dir.path());
    let hopeless = link_at(link.parent().unwrap(), "hopeless", "nowhere.txt");

    let run = run_with(&Options {
        verbose: true,
        ..with_prepend(dir.path(), "../")
    });

    assert_eq!(run.summary.fixed, 1);
    assert!(
        run.err.contains(&format!(
            "Attempting to fix by prepending: {}: target.txt -> ../target.txt\n",
            link.display()
        )),
        "the candidate that resolved is named: {:?}",
        run.err
    );
    assert!(
        run.err.contains(&format!(
            "Attempting to fix by prepending: {}: nowhere.txt -> ../nowhere.txt\n",
            hopeless.display()
        )),
        "the candidate that did not resolve is named: {:?}",
        run.err
    );
    assert!(
        run.err.contains(&format!(
            "Prepended target does not exist: {} -> ../nowhere.txt\n",
            hopeless.display()
        )),
        "the candidate that did not resolve is called absent: {:?}",
        run.err
    );
    assert!(
        !run.err.contains(&format!(
            "Prepended target does not exist: {} -> ../target.txt",
            link.display()
        )),
        "the candidate that resolved is not called absent: {:?}",
        run.err
    );
}

/// The options of a run that repairs by taking `prefix` off the front of a
/// target.
fn with_remove(root: &Path, prefix: impl Into<OsString>) -> Options {
    Options {
        remove: Some(prefix.into()),
        ..options(root)
    }
}

/// The line the tool writes for one repair the remove made.
fn removed_line(link: &Path, target: &str) -> String {
    format!(
        "Fixed symlink by removing prefix: {} -> {target}\n",
        link.display()
    )
}

/// Builds the tree the remove tests share: a link that names a real file under
/// a directory which is not there.
///
/// `{root}/sub/path/target.txt` is a file, and `{root}/sub/link` points at
/// `oldprefix/path/target.txt`, which resolves to
/// `{root}/sub/oldprefix/path/target.txt` and is not there. A remove of
/// `oldprefix/` repairs it.
fn carries_a_stale_prefix(root: &Path) -> PathBuf {
    let sub = dir_at(root, "sub");
    fs::write(dir_at(&sub, "path").join("target.txt"), b"contents").unwrap();
    link_at(&sub, "link", "oldprefix/path/target.txt")
}

#[test]
fn a_remove_that_resolves_repairs_the_link() {
    let dir = TempDir::new().unwrap();
    let link = carries_a_stale_prefix(dir.path());

    let run = run_with(&with_remove(dir.path(), "oldprefix/"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        fs::metadata(&link).is_ok(),
        "the repaired link resolves: {:?}",
        fs::read_link(&link)
    );
    assert!(
        run.out.contains(&removed_line(&link, "path/target.txt")),
        "the repair is reported: {:?}",
        run.out
    );
    assert!(
        run.out.ends_with("Fixed 1 symlink(s).\n"),
        "the report ends with the count of repairs: {:?}",
        run.out
    );
}

#[test]
fn a_repaired_link_holds_the_target_the_remove_built() {
    // The new link holds the candidate as the tool built it, so a repair that
    // took a prefix off a relative target leaves a relative target behind.
    let dir = TempDir::new().unwrap();
    let link = carries_a_stale_prefix(dir.path());

    let run = run_with(&with_remove(dir.path(), "oldprefix/"));

    assert_eq!(run.summary.fixed, 1);
    let target = fs::read_link(&link).unwrap();
    assert_eq!(target, Path::new("path/target.txt"));
    assert!(
        !target.is_absolute(),
        "the repaired target is still relative: {}",
        target.display()
    );
}

#[test]
fn a_target_without_the_prefix_gets_no_repair() {
    let dir = TempDir::new().unwrap();
    let link = link_at(dir.path(), "link", "missing.txt");

    let run = run_with(&with_remove(dir.path(), "oldprefix/"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 0,
            errors: 0
        }
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("missing.txt"));
    assert_eq!(
        run.out,
        format!(
            "{}Found 1 broken symlink(s).\n{NOTHING_FIXED}",
            broken_line(&link, "missing.txt")
        )
    );
}

#[test]
fn the_prefix_comes_off_by_bytes_and_not_by_path_components() {
    // The tool this port replaces asks `strings.HasPrefix`, which reads the
    // target as bytes. `Path::strip_prefix` compares whole components and would
    // refuse `old` on `oldpath/target.txt`, so this test pins that the port
    // keeps the behavior of the tool it replaces.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    fs::write(dir_at(&sub, "path").join("target.txt"), b"contents").unwrap();
    let link = link_at(&sub, "link", "oldpath/target.txt");

    let run = run_with(&with_remove(dir.path(), "old"));

    assert_eq!(run.summary.fixed, 1);
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("path/target.txt"),
        "the prefix came off mid-component"
    );
    assert!(
        run.out.contains(&removed_line(&link, "path/target.txt")),
        "the repair is reported: {:?}",
        run.out
    );
}

#[test]
fn the_prepend_is_tried_before_the_remove() {
    // Both strategies would repair this link, so the report says which one the
    // chain reached first. The tool this port replaces carries a `fixed`
    // boolean that its prepend branch sets and its remove branch never does;
    // here the order lives in one `or_else` chain and there is no flag beside
    // it to fall out of step.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    // The prepend candidate `../old/target.txt` resolves to `{T}/old/target.txt`.
    fs::write(dir_at(dir.path(), "old").join("target.txt"), b"contents").unwrap();
    // The remove candidate `target.txt` resolves to `{T}/sub/target.txt`.
    fs::write(sub.join("target.txt"), b"contents").unwrap();
    let link = link_at(&sub, "link", "old/target.txt");

    let run = run_with(&Options {
        remove: Some(OsString::from("old/")),
        ..with_prepend(dir.path(), "../")
    });

    assert_eq!(run.summary.fixed, 1);
    assert!(
        run.out
            .contains(&prepended_line(&link, "../old/target.txt")),
        "the prepend made the repair: {:?}",
        run.out
    );
    assert!(
        !run.out.contains("Fixed symlink by removing prefix:"),
        "the remove was never reached: {:?}",
        run.out
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("../old/target.txt"),
        "the link holds the candidate the prepend built"
    );
}

#[test]
fn the_remove_runs_when_the_prepend_does_not_resolve() {
    // The other half of the order: the remove is the next link of the chain,
    // not a branch that a repair which already happened has to switch off.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    fs::write(sub.join("target.txt"), b"contents").unwrap();
    let link = link_at(&sub, "link", "old/target.txt");

    let run = run_with(&Options {
        remove: Some(OsString::from("old/")),
        ..with_prepend(dir.path(), "nowhere/")
    });

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        run.out.contains(&removed_line(&link, "target.txt")),
        "the remove made the repair: {:?}",
        run.out
    );
    assert!(
        !run.out.contains("Fixed symlink by prepending:"),
        "the prepend made none: {:?}",
        run.out
    );
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("target.txt"));
}

#[test]
fn an_absolute_remove_candidate_the_link_would_not_resolve_to_is_refused() {
    // The remove reaches the same defect the prepend does, and this tree is
    // built so that the two spellings of the check disagree and the wrong one
    // succeeds.
    //
    // `{root}/sub/link` points at `/old/path/foo`, which is not there, so the
    // link is broken. `{root}/sub/path/foo` is a real file. A remove of `/old`
    // builds the candidate `/path/foo`.
    //
    // The Go tool checks `filepath.Join("{root}/sub", "/path/foo")`, and
    // `filepath.Join` appends an absolute second argument rather than replacing
    // the first. So it checks `{root}/sub/path/foo`, finds the file, writes
    // `/path/foo` into the link, prints `Fixed symlink by removing prefix:`,
    // and counts a repair — leaving a different broken link in place of the old
    // one.
    //
    // The operating system resolves an absolute link target against the root,
    // so the link would resolve to `/path/foo` and to nothing else.
    // `Path::join` replaces the first path when the second is absolute, which
    // is that same rule, thus this tool checks `/path/foo`, does not find it,
    // and refuses.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    let link = link_at(&sub, "link", "/old/path/foo");
    fs::write(dir_at(&sub, "path").join("foo"), b"contents").unwrap();
    // The test says what it means to say only while nothing on this machine
    // answers to either name. Nothing is written to either one: these are reads
    // of two names, and the only path the tool may write is the link inside the
    // temporary directory.
    assert!(
        fs::symlink_metadata("/old/path/foo").is_err(),
        "this test needs /old/path/foo to be absent, and it is not"
    );
    assert!(
        fs::symlink_metadata("/path/foo").is_err(),
        "this test needs /path/foo to be absent, and it is not"
    );

    let run = run_with(&with_remove(dir.path(), "/old"));

    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 0,
            errors: 0
        }
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("/old/path/foo"),
        "the link still holds the target it was made with"
    );
    assert!(
        !run.out.contains("Fixed"),
        "no repair is reported: {:?}",
        run.out
    );
    assert!(
        run.out.contains(NOTHING_FIXED),
        "the report says nothing could be repaired: {:?}",
        run.out
    );
}

#[test]
fn an_absolute_remove_candidate_that_resolves_repairs_the_link() {
    // The other half of the rule, for the remove. An absolute candidate is not
    // refused as a class: the tool refuses a candidate the link would not
    // resolve to, and accepts one it would, whatever shape it has.
    let dir = TempDir::new().unwrap();
    fs::write(dir_at(dir.path(), "absent").join("target.txt"), b"contents").unwrap();
    let sub = dir_at(dir.path(), "sub");
    let mut stale = OsString::from("/junk");
    stale.push(dir.path());
    stale.push("/absent/target.txt");
    let link = link_at(&sub, "link", &stale);
    assert!(
        fs::symlink_metadata(&stale).is_err(),
        "this test needs {stale:?} to be absent, and it is not"
    );

    let run = run_with(&with_remove(dir.path(), "/junk"));

    let expected = dir.path().join("absent").join("target.txt");
    assert_eq!(
        run.summary,
        Summary {
            broken: 1,
            fixed: 1,
            errors: 0
        }
    );
    assert!(
        fs::metadata(&link).is_ok(),
        "the repaired link resolves: {:?}",
        fs::read_link(&link)
    );
    assert_eq!(
        fs::read_link(&link).unwrap(),
        expected,
        "the link holds the absolute candidate as it was built"
    );
    assert!(
        run.out
            .contains(&removed_line(&link, &expected.display().to_string())),
        "the repair is reported: {:?}",
        run.out
    );
}

#[test]
fn a_target_that_is_not_utf8_is_stripped_by_bytes() {
    // The byte `0x80` never begins a UTF-8 sequence, so this target is not
    // text. A strip that went through `str` would refuse it and the tool would
    // make no repair, thus a repair here is the proof that the strip reads
    // bytes.
    //
    // Only the removed prefix carries that byte. The file the candidate names
    // has an ordinary name because APFS refuses a file name that is not valid
    // UTF-8, while it stores the target of a link byte for byte. The unit test
    // `a_target_that_is_not_utf8_keeps_its_bytes` in `src/pathbytes.rs` pins
    // that the bytes after the prefix come back untouched, which needs no file
    // system at all.
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    fs::write(sub.join("target.txt"), b"contents").unwrap();
    let stale = OsString::from_vec(b"junk\x80/target.txt".to_vec());
    let link = link_at(&sub, "link", &stale);
    assert_eq!(
        fs::read_link(&link).unwrap().as_os_str().as_bytes(),
        b"junk\x80/target.txt",
        "the file system kept the bytes of the target the fixture wrote"
    );

    let run = run_with(&with_remove(
        dir.path(),
        OsString::from_vec(b"junk\x80/".to_vec()),
    ));

    assert_eq!(run.summary.fixed, 1);
    assert_eq!(
        fs::read_link(&link).unwrap(),
        Path::new("target.txt"),
        "the candidate is what is left after the bytes of the prefix"
    );
}

#[test]
fn verbose_names_the_removed_candidate_and_calls_it_absent() {
    // The two debug lines are the only account a user gets of a candidate the
    // remove built and then refused, so they name the candidate and not just
    // the link.
    let dir = TempDir::new().unwrap();
    let sub = dir_at(dir.path(), "sub");
    let link = link_at(&sub, "link", "oldprefix/nowhere.txt");

    let run = run_with(&Options {
        verbose: true,
        ..with_remove(dir.path(), "oldprefix/")
    });

    assert_eq!(run.summary.fixed, 0);
    assert!(
        run.err.contains(&format!(
            "Attempting to fix by removing prefix: {}: oldprefix/nowhere.txt -> nowhere.txt\n",
            link.display()
        )),
        "the candidate the remove built is named: {:?}",
        run.err
    );
    assert!(
        run.err.contains(&format!(
            "Target with removed prefix does not exist: {} -> nowhere.txt\n",
            link.display()
        )),
        "the candidate that did not resolve is called absent: {:?}",
        run.err
    );
}
