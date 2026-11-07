//! The operations that `bibibi` needs from `gitscratch`: two reads on `Repo`
//! and four operations on `Scratch`.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use gitscratch::testing::TestRepo;
use gitscratch::{NoInheritedGitEnvironment, Repo};
use tempfile::TempDir;

/// Open the fixture as a `Repo`.
fn open(fixture: &TestRepo) -> Repo {
    Repo::open(fixture.path()).expect("a fixture is a git repository")
}

/// A fixture with two commits. The second one changes `a.txt` and adds `b.txt`.
fn two_commit_repo() -> TestRepo {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\ntwo\nthree\n", "first");
    fixture.commit_files(
        &[("a.txt", "one\n2\nthree\n"), ("b.txt", "new file\n")],
        "second",
    );
    fixture
}

/// Check that `patch` turns the parent of HEAD into the tree of HEAD.
///
/// The patch goes to a file and `git apply` reads it in a second checkout of
/// the parent. That is the use that `patch_of` exists for.
fn assert_patch_rebuilds_head(fixture: &TestRepo, patch: &[u8]) {
    let holder = TempDir::new().expect("create temp dir");
    let checkout = holder.path().join("checkout");
    let patch_file = holder.path().join("change.patch");
    std::fs::write(&patch_file, patch).expect("write the patch");

    let checkout_arg = checkout.to_str().expect("UTF-8 path");
    fixture.git(&["worktree", "add", "-q", "--detach", checkout_arg, "HEAD~1"]);

    let patch_arg = patch_file.to_str().expect("UTF-8 path");
    let applied = fixture.try_git(&["-C", checkout_arg, "apply", patch_arg], &[]);
    assert!(
        applied.status.success(),
        "git apply refused the patch: {}",
        String::from_utf8_lossy(&applied.stderr)
    );

    fixture.git(&["-C", checkout_arg, "add", "-A"]);
    let rebuilt = fixture.git(&["-C", checkout_arg, "write-tree"]);
    assert_eq!(rebuilt, fixture.rev_parse("HEAD^{tree}"));
}

#[test]
fn patch_of_applies_to_the_parent_and_gives_the_tree_of_the_commit() {
    let fixture = two_commit_repo();

    let patch = open(&fixture).patch_of("HEAD").expect("a patch");

    assert_patch_rebuilds_head(&fixture, &patch);
}

#[test]
fn patch_of_ignores_color_and_prefix_configuration() {
    let fixture = two_commit_repo();
    fixture.git(&["config", "color.ui", "always"]);
    fixture.git(&["config", "diff.noprefix", "true"]);

    let patch = open(&fixture).patch_of("HEAD").expect("a patch");

    assert!(
        !patch.contains(&0x1b),
        "the patch holds an ANSI escape byte"
    );
    let text = String::from_utf8(patch.clone()).expect("a text patch");
    assert!(text.contains("--- a/a.txt"), "no a/ prefix: {text}");
    assert!(text.contains("+++ b/a.txt"), "no b/ prefix: {text}");
    assert_patch_rebuilds_head(&fixture, &patch);
}

#[test]
fn subject_gives_the_first_line_of_the_message() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "add the first file");

    let subject = open(&fixture).subject("HEAD").expect("a subject");

    assert_eq!(subject, "add the first file");
}

#[test]
fn subject_keeps_multi_byte_characters() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "日本語 🎉 café");

    let subject = open(&fixture).subject("HEAD").expect("a subject");

    assert_eq!(subject, "日本語 🎉 café");
}

/// Write `patch` to a file in a new temporary directory.
///
/// The directory comes back with the path, because its drop removes the file.
fn patch_file(patch: &[u8]) -> (TempDir, std::path::PathBuf) {
    let holder = TempDir::new().expect("create temp dir");
    let path = holder.path().join("change.patch");
    std::fs::write(&path, patch).expect("write the patch");
    (holder, path)
}

#[test]
fn apply_patch_applies_a_patch_that_fits() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "first");
    fixture.commit_file("b.txt", "added by the patch\n", "second");
    let patch = open(&fixture).patch_of("HEAD").expect("a patch");
    let (_holder, path) = patch_file(&patch);
    let scratch = fixture.scratch("HEAD~1");

    let applied = scratch.apply_patch(&path).expect("git started");

    assert!(applied, "the patch fits and has to apply");
    let contents = std::fs::read_to_string(scratch.path().join("b.txt")).expect("b.txt exists");
    assert_eq!(contents, "added by the patch\n");
}

#[test]
fn apply_patch_gives_false_and_changes_nothing_when_the_patch_does_not_fit() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "first");
    fixture.commit_file("a.txt", "two\n", "second");
    let patch = open(&fixture).patch_of("HEAD").expect("a patch");
    let (_holder, path) = patch_file(&patch);
    fixture.commit_file("a.txt", "three\n", "third");
    let scratch = fixture.scratch("HEAD");

    let applied = scratch.apply_patch(&path).expect("git started");

    assert!(!applied, "the patch expects 'one' and has to be refused");
    let contents = std::fs::read_to_string(scratch.path().join("a.txt")).expect("a.txt exists");
    assert_eq!(contents, "three\n");
    let root = scratch.path().to_str().expect("UTF-8 path");
    assert_eq!(fixture.git(&["-C", root, "status", "--porcelain"]), "");
}

#[test]
fn apply_patch_ignores_a_whitespace_policy_of_the_user() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "first");
    // The line ends in a space. Git commits it without complaint.
    fixture.commit_file("a.txt", "one\ntrailing space \n", "second");
    let patch = open(&fixture).patch_of("HEAD").expect("a patch");
    let (_holder, path) = patch_file(&patch);
    fixture.git(&["config", "apply.whitespace", "error"]);
    let scratch = fixture.scratch("HEAD~1");

    let applied = scratch.apply_patch(&path).expect("git started");

    assert!(applied, "apply.whitespace=error must not refuse the patch");
}

#[test]
fn restore_removes_untracked_files_and_directories() {
    let fixture = TestRepo::init();
    fixture.commit_file("a.txt", "one\n", "first");
    let scratch = fixture.scratch("HEAD");
    std::fs::write(scratch.path().join("stray.txt"), "x\n").expect("write a file");
    std::fs::create_dir_all(scratch.path().join("stray-dir/inner")).expect("make directories");
    std::fs::write(scratch.path().join("stray-dir/inner/f.txt"), "y\n").expect("write a file");

    scratch.restore().expect("restore");

    assert!(!scratch.path().join("stray.txt").exists());
    assert!(!scratch.path().join("stray-dir").exists());
    assert!(
        scratch.path().join("a.txt").exists(),
        "a tracked file stays"
    );
}

#[test]
fn restore_keeps_ignored_files() {
    let fixture = TestRepo::init();
    fixture.commit_files(
        &[(".gitignore", "target/\n*.o\n"), ("a.txt", "one\n")],
        "first",
    );
    let scratch = fixture.scratch("HEAD");
    std::fs::create_dir_all(scratch.path().join("target")).expect("make a directory");
    std::fs::write(scratch.path().join("target/out.bin"), "built\n").expect("write a file");
    std::fs::write(scratch.path().join("main.o"), "object\n").expect("write a file");
    std::fs::write(scratch.path().join("stray.txt"), "x\n").expect("write a file");

    scratch.restore().expect("restore");

    assert!(scratch.path().join("target/out.bin").exists());
    assert!(scratch.path().join("main.o").exists());
    assert!(!scratch.path().join("stray.txt").exists());
}

/// Make a commit that is not the first and give back its id.
fn commit_numbered(fixture: &TestRepo, number: usize) -> String {
    fixture.commit_file(
        &format!("f{number}.txt"),
        &format!("{number}\n"),
        &format!("commit {number}"),
    );
    fixture.rev_parse("HEAD")
}

/// Check that `item` is a full object id: 40 hexadecimal digits and nothing else.
fn assert_full_id(item: &str) {
    assert_eq!(item.len(), 40, "not a full id: {item:?}");
    assert!(
        item.chars().all(|c| c.is_ascii_hexdigit()),
        "not a hexadecimal id: {item:?}"
    );
}

#[test]
fn bisect_all_lists_the_commits_after_the_good_one_up_to_the_bad_one() {
    let fixture = TestRepo::init();
    let ids: Vec<String> = (1..=5).map(|n| commit_numbered(&fixture, n)).collect();
    let scratch = fixture.scratch(&ids[4]);

    let listed = scratch
        .bisect_all(&ids[4], &[ids[0].as_str()])
        .expect("a list");

    // The decoration of the line, for example "(HEAD -> main, dist=0)", is gone.
    for item in &listed {
        assert_full_id(item);
    }
    let found: std::collections::HashSet<&str> = listed.iter().map(String::as_str).collect();
    let expected: std::collections::HashSet<&str> = ids[1..].iter().map(String::as_str).collect();
    assert_eq!(found, expected);
    assert_eq!(listed.len(), 4, "no commit is listed twice");
}

#[test]
fn bisect_all_excludes_the_commits_of_every_good_branch() {
    let fixture = TestRepo::init();
    let base = commit_numbered(&fixture, 1);
    fixture.git(&["checkout", "-q", "-b", "left"]);
    let left = commit_numbered(&fixture, 2);
    fixture.git(&["checkout", "-q", "main"]);
    fixture.git(&["checkout", "-q", "-b", "right"]);
    let right = commit_numbered(&fixture, 3);
    fixture.git(&["merge", "-q", "--no-ff", "-m", "merge", "left"]);
    let merge = fixture.rev_parse("HEAD");
    let after_one = commit_numbered(&fixture, 4);
    let after_two = commit_numbered(&fixture, 5);
    let scratch = fixture.scratch(&after_two);

    let listed = scratch
        .bisect_all(&after_two, &[left.as_str(), right.as_str()])
        .expect("a list");

    let found: std::collections::HashSet<&str> = listed.iter().map(String::as_str).collect();
    let expected: std::collections::HashSet<&str> = [&merge, &after_one, &after_two]
        .into_iter()
        .map(String::as_str)
        .collect();
    assert_eq!(found, expected, "the base ({base}) and both goods are out");
}

#[test]
fn bisect_steps_is_zero_when_only_the_bad_commit_is_left() {
    let fixture = TestRepo::init();
    let ids: Vec<String> = (1..=2).map(|n| commit_numbered(&fixture, n)).collect();
    let scratch = fixture.scratch(&ids[1]);

    let steps = scratch
        .bisect_steps(&ids[1], &[ids[0].as_str()])
        .expect("a count");

    assert_eq!(steps, 0);
}

#[test]
fn bisect_steps_matches_the_count_that_git_gives_for_a_longer_range() {
    let fixture = TestRepo::init();
    let ids: Vec<String> = (1..=9).map(|n| commit_numbered(&fixture, n)).collect();
    let scratch = fixture.scratch(&ids[8]);

    let steps = scratch
        .bisect_steps(&ids[8], &[ids[0].as_str()])
        .expect("a count");

    // The range holds commits 2 to 9, which is 8 commits. The literal comes
    // from a shell run with git 2.56: `git rev-list --bisect-vars` over 8
    // commits in a line prints `bisect_all=8` and `bisect_steps=2`. For
    // comparison, 1 or 2 commits give 0, 3 to 5 give 1, and 12 give 3.
    assert_eq!(steps, 2);
}

#[test]
fn restore_puts_back_a_changed_tracked_file_and_unstages_an_index_change() {
    let fixture = TestRepo::init();
    fixture.commit_files(&[("a.txt", "one\n"), ("b.txt", "two\n")], "first");
    let scratch = fixture.scratch("HEAD");
    std::fs::write(scratch.path().join("a.txt"), "changed in the tree\n").expect("write a file");
    std::fs::write(scratch.path().join("b.txt"), "changed in the index\n").expect("write a file");
    let staged = std::process::Command::new("git")
        .without_inherited_git_environment()
        .args(["add", "b.txt"])
        .current_dir(scratch.path())
        .status()
        .expect("git started");
    assert!(
        staged.success(),
        "git add must work in the scratch worktree"
    );

    scratch.restore().expect("restore");

    let status = std::process::Command::new("git")
        .without_inherited_git_environment()
        .args(["status", "--porcelain"])
        .current_dir(scratch.path())
        .output()
        .expect("git started");
    assert_eq!(String::from_utf8_lossy(&status.stdout), "");
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("a.txt")).expect("read a.txt"),
        "one\n"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("b.txt")).expect("read b.txt"),
        "two\n"
    );
}
