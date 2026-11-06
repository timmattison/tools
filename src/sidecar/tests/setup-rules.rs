#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! The rules of `sidecar setup`, tested through the binary.

mod support;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Stdio;

use base64::Engine;

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

#[test]
fn rule_5_points_a_broken_link_at_the_main_clone() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let link = fixture.link(&host, NAME);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(fixture.root().join("does-not-exist"), &link).unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(
        fs::read_link(&link).unwrap(),
        canon(&fixture.main_clone(NAME))
    );
    assert!(
        run.stdout
            .contains("Pointed the broken link data/testdata at "),
        "{run:?}"
    );
}

/// Asserts that the run refused to touch the link and said so.
fn assert_warned_about_the_link(run: &support::Run) {
    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stderr
            .lines()
            .any(|line| line.starts_with("warning: ") && line.contains("data/testdata")),
        "{run:?}"
    );
}

#[test]
fn rule_6_does_not_touch_a_real_directory_at_the_link() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let link = fixture.link(&host, NAME);
    fixture.write(&link.join("keep.txt"), "mine\n");

    let run = fixture.run(&host, &["setup"]);

    assert_warned_about_the_link(&run);
    assert_eq!(fs::read_to_string(link.join("keep.txt")).unwrap(), "mine\n");
    assert!(!fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[test]
fn rule_6_does_not_touch_a_regular_file_at_the_link() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let link = fixture.link(&host, NAME);
    fixture.write(&link, "mine\n");

    let run = fixture.run(&host, &["setup"]);

    assert_warned_about_the_link(&run);
    assert_eq!(fs::read_to_string(&link).unwrap(), "mine\n");
}

#[test]
fn rule_6_does_not_touch_a_link_to_a_different_directory() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let elsewhere = fixture.root().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let link = fixture.link(&host, NAME);
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&elsewhere, &link).unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_warned_about_the_link(&run);
    assert_eq!(fs::read_link(&link).unwrap(), elsewhere);
}

#[test]
fn rule_2_warns_about_a_foreign_directory_at_the_main_clone() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let place = fixture.main_clone(NAME);
    fixture.write(&place.join("other.txt"), "not a clone\n");

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stderr
            .lines()
            .any(|line| line.starts_with("warning: ") && line.contains("is not a clone of")),
        "{run:?}"
    );
    assert!(place.join("other.txt").is_file());
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_err());
}

#[test]
fn rule_2_warns_about_a_clone_that_did_not_end() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let place = fixture.main_clone(NAME);
    fixture.git(
        fixture.root(),
        &[
            "clone",
            "--quiet",
            "--no-checkout",
            &fixture.sidecar_remote(NAME).to_string_lossy(),
            &place.to_string_lossy(),
        ],
    );

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stderr
            .lines()
            .any(|line| line.starts_with("warning: ") && line.contains("sidecar setup")),
        "{run:?}"
    );
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_err());
}

#[test]
fn rule_8_warns_when_the_remote_has_no_main_branch() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fs::remove_dir_all(fixture.sidecar_remote(NAME)).unwrap();
    fixture.make_empty_sidecar_remote(NAME);

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stderr.lines().any(|line| line.starts_with("warning: ")
            && line.contains("megarepo-data-testdata")
            && line.contains("sidecar setup")),
        "{run:?}"
    );
    assert!(!fixture.main_clone(NAME).exists());
    assert!(fs::symlink_metadata(fixture.link(&host, NAME)).is_err());
    let leftovers: Vec<_> = fs::read_dir(fixture.root().join("code"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("sidecar-clone"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn rule_8_continues_with_the_next_name_when_a_clone_fails() {
    let fixture = Fixture::new(&[NAME, "pics"]);
    let host = fixture.main_checkout();
    fs::rename(
        fixture.sidecar_remote(NAME),
        fixture.root().join("moved-away-testdata.git"),
    )
    .unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(
        run.stderr.lines().any(|line| line.starts_with("warning: ")
            && line.contains("testdata")
            && line.contains("sidecar setup")),
        "{run:?}"
    );
    assert!(!fixture.main_clone(NAME).exists());
    assert!(fixture.main_clone("pics").join("data.txt").is_file());
    assert_eq!(
        canon(&fixture.link(&host, "pics")),
        canon(&fixture.main_clone("pics"))
    );
}

#[test]
fn a_second_run_makes_no_network_call_and_writes_nothing() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    let first = fixture.run(&host, &["setup"]);
    assert_eq!(first.code, 0, "{first:?}");
    fs::rename(
        fixture.sidecar_remote(NAME),
        fixture.root().join("moved-away-testdata.git"),
    )
    .unwrap();

    let second = fixture.run(&host, &["setup"]);

    assert_eq!(second.code, 0, "{second:?}");
    assert_eq!(second.stdout, "", "{second:?}");
    assert_eq!(second.stderr, "", "{second:?}");
}

/// The names in `dir` that start with `prefix`.
fn entries_starting_with(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect()
}

#[test]
fn two_runs_at_the_same_time_make_one_main_clone() {
    for round in 0..3 {
        let fixture = Fixture::new(&[NAME]);
        let first = fixture.add_host_worktree("first");
        let second = fixture.add_host_worktree("second");

        let child_one = fixture
            .sidecar(&first, &["setup"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let child_two = fixture
            .sidecar(&second, &["setup"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output_one = child_one.wait_with_output().unwrap();
        let output_two = child_two.wait_with_output().unwrap();

        for output in [&output_one, &output_two] {
            assert_eq!(output.status.code(), Some(0), "round {round}: {output:?}");
        }
        let code_dir = fixture.root().join("code");
        assert_eq!(
            entries_starting_with(&code_dir, "megarepo-data-testdata"),
            vec!["megarepo-data-testdata".to_string()],
            "round {round}"
        );
        assert!(
            entries_starting_with(&code_dir, ".megarepo-data-testdata.sidecar-clone-").is_empty(),
            "round {round}"
        );
        for worktree in [&first, &second] {
            assert_eq!(
                canon(&fixture.link(worktree, NAME)),
                canon(&fixture.main_clone(NAME)),
                "round {round}"
            );
        }
    }
}

/// Runs setup with the token `token` in the variable `variable` and checks that the
/// token is in no output, no config file, and no remote URL.
fn assert_token_stays_secret(fixture: &Fixture, host: &Path, variable: &str, token: &str) {
    let run = fixture.run_with_env(host, &["setup"], &[(variable, token)]);
    assert_eq!(run.code, 0, "{run:?}");
    let clone = fixture.main_clone(NAME);
    assert!(clone.join("data.txt").is_file());
    let encoded =
        base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    let config =
        String::from_utf8_lossy(&fs::read(clone.join(".git").join("config")).unwrap()).into_owned();
    let remotes = fixture.git(&clone, &["remote", "-v"]);
    for (place, text) in [
        ("stdout", &run.stdout),
        ("stderr", &run.stderr),
        (".git/config", &config),
        ("git remote -v", &remotes),
    ] {
        assert!(!text.contains(token), "the token is in {place}: {text}");
        assert!(
            !text.contains(&encoded),
            "the encoded token is in {place}: {text}"
        );
    }
}

/// A token value that no other run uses.
fn unique_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("tok-{}-{nanos}", std::process::id())
}

#[test]
fn a_token_is_never_in_the_output_the_config_or_a_remote_url() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    assert_token_stays_secret(&fixture, &host, "SIDECAR_TOKEN", &unique_token());
}

#[test]
fn a_token_from_a_named_variable_is_never_in_the_output_the_config_or_a_remote_url() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fixture.write(
        &host.join("sidecar.toml"),
        "token_env = \"MY_DATA_TOKEN\"\nsidecars = [\"testdata\"]\n",
    );
    assert_token_stays_secret(&fixture, &host, "MY_DATA_TOKEN", &unique_token());
}

#[test]
fn without_hook_an_unlinked_sidecar_gives_exit_1() {
    let fixture = Fixture::new(&[NAME]);
    let host = fixture.main_checkout();
    fs::rename(
        fixture.sidecar_remote(NAME),
        fixture.root().join("moved-away-testdata.git"),
    )
    .unwrap();

    let run = fixture.run(&host, &["setup"]);

    assert_eq!(run.code, 1, "{run:?}");
    assert!(!fixture.main_clone(NAME).exists());
}
