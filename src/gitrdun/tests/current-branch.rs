//! Holds the branch name that `gitrdun` shows for a repository.
//!
//! `get_current_branch` gives the short name of the branch that `HEAD` points
//! at. It gives `"HEAD"` when there is no such name: a detached `HEAD`, a
//! branch with no commit, a path that is not a repository, and a branch name
//! that is not UTF-8.
//!
//! Each fixture is made with `git2` in its own temporary directory. No test
//! starts a git child process, thus no `GIT_` variable of a hook can send a
//! fixture to a different repository.

use git2::{Oid, Repository, RepositoryInitOptions, Signature};
use gitrdun::git::get_current_branch;
use std::path::Path;
use tempfile::TempDir;

/// Makes a repository whose `HEAD` is the branch `branch`, with no commit.
fn init_on_branch(dir: &Path, branch: &str) -> Repository {
    let mut options = RepositoryInitOptions::new();
    options.initial_head(branch);
    Repository::init_opts(dir, &options).expect("the fixture repository must initialize")
}

/// Adds one commit with an empty tree on the branch that `HEAD` points at.
fn commit_empty_tree(repo: &Repository) -> Oid {
    let signature =
        Signature::now("Fixture", "fixture@example.com").expect("the signature must be valid");
    let tree_id = repo
        .index()
        .and_then(|mut index| index.write_tree())
        .expect("the empty tree must write");
    let tree = repo.find_tree(tree_id).expect("the empty tree must exist");
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .expect("the fixture commit must write")
}

#[test]
fn a_branch_gives_its_short_name() {
    let dir = TempDir::new().expect("the temporary directory must exist");
    let repo = init_on_branch(dir.path(), "feature/日本語-🎉-café");
    commit_empty_tree(&repo);

    assert_eq!(get_current_branch(dir.path()), "feature/日本語-🎉-café");
}

#[test]
fn a_detached_head_gives_head() {
    let dir = TempDir::new().expect("the temporary directory must exist");
    let repo = init_on_branch(dir.path(), "main");
    let oid = commit_empty_tree(&repo);
    repo.set_head_detached(oid)
        .expect("HEAD must detach at the fixture commit");

    assert_eq!(get_current_branch(dir.path()), "HEAD");
}

#[test]
fn a_branch_with_no_commit_gives_head() {
    let dir = TempDir::new().expect("the temporary directory must exist");
    init_on_branch(dir.path(), "main");

    assert_eq!(get_current_branch(dir.path()), "HEAD");
}

#[test]
fn a_path_that_is_not_a_repository_gives_head() {
    let dir = TempDir::new().expect("the temporary directory must exist");

    assert_eq!(get_current_branch(dir.path()), "HEAD");
}

/// A ref name can hold bytes that are not UTF-8. `git2` 0.20 gives `None` for
/// the short name of such a ref, and `git2` 0.21 gives an error. Both must
/// give `"HEAD"`.
///
/// The ref lives only in `packed-refs`, because the file system of a Mac
/// refuses a file name that is not UTF-8.
#[test]
fn a_branch_name_that_is_not_utf8_gives_head() {
    let dir = TempDir::new().expect("the temporary directory must exist");
    let repo = init_on_branch(dir.path(), "main");
    let oid = commit_empty_tree(&repo);

    let git_dir = repo.path();
    let ref_name: &[u8] = b"refs/heads/caf\xe9";
    let mut packed_refs = b"# pack-refs with: peeled fully-peeled sorted \n".to_vec();
    packed_refs.extend_from_slice(oid.to_string().as_bytes());
    packed_refs.push(b' ');
    packed_refs.extend_from_slice(ref_name);
    packed_refs.push(b'\n');
    std::fs::write(git_dir.join("packed-refs"), packed_refs).expect("packed-refs must write");

    let mut head = b"ref: ".to_vec();
    head.extend_from_slice(ref_name);
    head.push(b'\n');
    std::fs::write(git_dir.join("HEAD"), head).expect("HEAD must write");
    std::fs::remove_file(git_dir.join("refs/heads/main")).expect("the loose ref must go");

    let reopened = Repository::open(dir.path()).expect("the fixture must open again");
    let head = reopened
        .head()
        .expect("HEAD must resolve through packed-refs, or the test proves nothing");
    assert_eq!(head.name_bytes(), ref_name);

    assert_eq!(get_current_branch(dir.path()), "HEAD");
}
