//! The operations that `bibibi` needs from `gitscratch`: two reads on `Repo`
//! and four operations on `Scratch`.
//!
//! Every fixture lives in its own `TempDir`, so concurrent `cargo test` runs
//! never share a path.

use gitscratch::testing::TestRepo;
use gitscratch::Repo;
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
