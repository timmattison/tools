#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! `sidecar path <name>`, tested through the binary.

mod support;

use std::fs;
use std::path::Path;

use support::Fixture;

/// The name of the sidecar in these tests.
const NAME: &str = "testdata";

/// Makes a fixture and runs `sidecar setup` in the main checkout.
fn after_setup() -> Fixture {
    let fixture = Fixture::new(&[NAME]);
    let run = fixture.run(&fixture.main_checkout(), &["setup"]);
    assert_eq!(run.code, 0, "{run:?}");
    fixture
}

/// Asserts that the output of `sidecar path` names the link at `link`.
fn assert_prints_link(stdout: &str, link: &Path) {
    assert!(stdout.ends_with('\n'), "{stdout:?}");
    let printed = Path::new(stdout.trim_end());
    assert!(printed.is_absolute(), "{stdout:?}");
    assert_eq!(
        fs::canonicalize(printed.parent().unwrap()).unwrap(),
        fs::canonicalize(link.parent().unwrap()).unwrap()
    );
    assert_eq!(printed.file_name(), link.file_name());
}

#[test]
fn a_link_at_the_main_clone_prints_the_path_of_the_link() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let run = fixture.run(&host, &["path", NAME]);
    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(run.stderr, "");
    assert_prints_link(&run.stdout, &fixture.link(&host, NAME));
}

#[test]
fn a_link_at_a_sidecar_worktree_prints_the_path_of_the_link() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let place = fixture.worktrees_dir(NAME).join("feature");
    fixture.git(
        &fixture.main_clone(NAME),
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            &place.to_string_lossy(),
        ],
    );
    let link = fixture.link(&host, NAME);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&place, &link).unwrap();

    let run = fixture.run(&host, &["path", NAME]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_prints_link(&run.stdout, &link);
}

#[test]
fn path_works_from_a_subdirectory_of_the_host_worktree() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let inner = host.join("some").join("deep");
    fs::create_dir_all(&inner).unwrap();
    let run = fixture.run(&inner, &["path", NAME]);
    assert_eq!(run.code, 0, "{run:?}");
    assert_prints_link(&run.stdout, &fixture.link(&host, NAME));
}

#[test]
fn a_name_that_the_registry_does_not_name_gives_exit_1_and_names_add() {
    let fixture = after_setup();
    let run = fixture.run(&fixture.main_checkout(), &["path", "other"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("sidecar add other"), "{run:?}");
}

#[test]
fn an_invalid_name_gives_exit_1_and_names_add() {
    let fixture = after_setup();
    let run = fixture.run(&fixture.main_checkout(), &["path", "Bad_Name"]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("sidecar add Bad_Name"), "{run:?}");
}

#[test]
fn a_missing_registry_gives_exit_1_and_names_add() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fixture.git(&host, &["rm", "--quiet", "sidecar.toml"]);
    let run = fixture.run(&host, &["path", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("sidecar add testdata"), "{run:?}");
}

#[test]
fn a_missing_link_gives_exit_1_and_names_setup() {
    let fixture = Fixture::new(&[NAME]);
    let run = fixture.run(&fixture.main_checkout(), &["path", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("sidecar setup"), "{run:?}");
}

#[test]
fn a_broken_link_gives_exit_1_and_names_setup() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let link = fixture.link(&host, NAME);
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(fixture.root().join("gone"), &link).unwrap();
    let run = fixture.run(&host, &["path", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("sidecar setup"), "{run:?}");
}

#[test]
fn a_foreign_link_gives_exit_1_and_prints_nothing_on_stdout() {
    let fixture = after_setup();
    let host = fixture.main_checkout();
    let link = fixture.link(&host, NAME);
    fs::remove_file(&link).unwrap();
    fs::create_dir(&link).unwrap();
    let run = fixture.run(&host, &["path", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert_eq!(run.stdout, "");
    assert!(run.stderr.starts_with("sidecar: "), "{run:?}");
}

#[test]
fn path_makes_no_directory() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let run = fixture.run(&host, &["path", NAME]);
    assert_eq!(run.code, 1, "{run:?}");
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_err());
    assert!(!host.join("data").exists());
    assert!(!fixture.main_clone(NAME).exists());
}
