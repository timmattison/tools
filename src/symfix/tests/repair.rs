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
//! Two of the tests below build a candidate target that begins with a slash.
//! That is the case the Go tool this port replaces gets wrong, and it is the
//! reason this file exists. `filepath.Join` appends an absolute second argument
//! to the first, while the operating system resolves an absolute link target
//! against the root and ignores the directory the link sits in. So the Go tool
//! can check one file, write the name of another into the link, and report a
//! repair it did not make. The two tests pin both halves of the rule: a
//! candidate the link would not resolve to is refused however plausible it
//! looks from the directory of the link, and an absolute candidate that does
//! resolve is accepted, because it is absence and not shape that a repair
//! answers.

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
