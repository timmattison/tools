//! `find_main_repo` answers with the repository it stands in, not with the
//! repository that an inherited `GIT_DIR` names.
//!
//! The function reads the current directory of the process, and a test must not
//! change that directory or set a variable in its own process. The test thus
//! starts the test binary again. The parent test builds a fixture with a linked
//! worktree and a decoy. It starts the child in the linked worktree with the
//! hostile variables aimed at the decoy. The child calls `find_main_repo` and
//! checks that the answer is the main worktree of the fixture. The parent then
//! checks that the decoy is byte-identical.
#![cfg(unix)]

use std::path::PathBuf;

use gitscratch::testing::{DecoyRepository, TestRepo};

/// The variable that tells the test it runs as the child.
const MARKER: &str = "REPOWALKER_HOSTILE_CHILD";

/// The variable that carries the expected answer to the child.
const EXPECTED: &str = "REPOWALKER_HOSTILE_EXPECTED";

/// The name of this test, as the test binary spells it.
const TEST_NAME: &str = "find_main_repo_answers_with_the_repository_it_stands_in";

#[test]
fn find_main_repo_answers_with_the_repository_it_stands_in() {
    if std::env::var_os(MARKER).is_some() {
        let expected = PathBuf::from(std::env::var_os(EXPECTED).expect("the expected answer"));
        let answer = repowalker::find_main_repo().expect("find_main_repo found a repository");

        assert_eq!(
            answer.canonicalize().expect("canonicalize the answer"),
            expected
                .canonicalize()
                .expect("canonicalize the expected path"),
        );
        return;
    }

    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "a\n", "first");
    let trunk = fixture.git(&["branch", "--show-current"]);
    fixture.branch("linked");
    fixture.checkout(&trunk);
    let linked = fixture.add_worktree("linked");

    let decoy_repo = TestRepo::init();
    decoy_repo.commit_file("decoy.txt", "decoy\n", "decoy");
    let decoy = DecoyRepository::of(decoy_repo);

    let mut command = childenv::fixture_command(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", TEST_NAME, "--nocapture"])
        .current_dir(&linked)
        .env(MARKER, "1")
        .env(EXPECTED, fixture.path());
    decoy.aim(&mut command);
    let output = command.output().expect("start the test binary again");

    assert!(
        output.status.success(),
        "the child test failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    decoy.assert_untouched("repowalker");
}
