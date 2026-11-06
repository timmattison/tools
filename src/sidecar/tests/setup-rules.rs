#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! The rules of `sidecar setup`, tested through the binary.

mod support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use support::Fixture;

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Canonicalizes a path that must exist.
fn canon(path: &Path) -> std::path::PathBuf {
    fs::canonicalize(path).unwrap_or_else(|e| panic!("{} must exist: {e}", path.display()))
}

#[test]
fn rule_1_clones_a_missing_main_clone_on_main() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    let clone = fixture.main_clone(NAME);
    assert_eq!(
        fixture.git(&clone, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
    let origin = fixture.git(&clone, &["remote", "get-url", "origin"]);
    assert_eq!(
        canon(Path::new(&origin)),
        canon(&fixture.sidecar_remote(NAME))
    );
    assert!(clone.join("data.txt").is_file());
    assert!(run.stdout.contains("Cloned "), "{run:?}");
    assert!(run.stdout.contains("Linked data/testdata to "), "{run:?}");
}

#[test]
fn rule_1_in_a_host_worktree_clones_next_to_the_main_checkout() {
    let fixture = Fixture::new(&[NAME]);
    let worktree = fixture.add_host_worktree("feature");

    let run = fixture.run(&worktree, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert!(fixture.main_clone(NAME).join("data.txt").is_file());
    assert!(!worktree
        .parent()
        .unwrap()
        .join("megarepo-data-testdata")
        .exists());
    let link = fixture.link(&worktree, NAME);
    assert_eq!(canon(&link), canon(&fixture.main_clone(NAME)));
    assert!(!fixture.link(&fixture.main_checkout(), NAME).exists());
}

#[test]
fn rule_3_makes_a_symbolic_link_with_the_absolute_path_of_the_main_clone() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    let link = fixture.link(&host, NAME);
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_link(&link).unwrap(),
        canon(&fixture.main_clone(NAME))
    );
}

#[test]
fn ignore_refusal_stops_before_any_clone_or_link() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fixture.write(&host.join(".gitignore"), "");

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.stderr.contains("/data/"), "{run:?}");
    assert!(!fixture.main_clone(NAME).exists());
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_err());
}

#[test]
fn setup_in_a_subdirectory_works_like_setup_at_the_root() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let subdirectory = host.join("deep").join("er");
    fs::create_dir_all(&subdirectory).unwrap();

    let run = fixture.run(&subdirectory, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        canon(&fixture.link(&host, NAME)),
        canon(&fixture.main_clone(NAME))
    );
    assert!(!fixture.link(&subdirectory, NAME).exists());
}

/// Runs setup once so that the main clone and the link exist.
fn set_up(fixture: &Fixture, host: &Path) {
    let run = fixture.run(host, &["setup"]);
    assert_eq!(run.code, 0, "{run:?}");
}

#[test]
fn rule_4a_leaves_a_correct_link_as_it_is() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    set_up(&fixture, &host);
    let link = fixture.link(&host, NAME);
    let before = fs::read_link(&link).unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(run.stdout, "", "{run:?}");
    assert_eq!(run.stderr, "", "{run:?}");
    assert_eq!(fs::read_link(&link).unwrap(), before);
}

#[test]
fn rule_4b_leaves_a_link_to_a_worktree_of_the_main_clone() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    set_up(&fixture, &host);
    let clone = fixture.main_clone(NAME);
    let sidecar_worktree = fixture.worktrees_dir(NAME).join("feature");
    fixture.git(
        &clone,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            &sidecar_worktree.to_string_lossy(),
        ],
    );
    let link = fixture.link(&host, NAME);
    fs::remove_file(&link).unwrap();
    symlink(&sidecar_worktree, &link).unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(run.stdout, "", "{run:?}");
    assert_eq!(fs::read_link(&link).unwrap(), sidecar_worktree);
}
