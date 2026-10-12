//! Holds the workspace-wide rules for the `libgit2` that `git2` links.
//!
//! The rules live on the `git2` entry of `[workspace.dependencies]` in the
//! root `Cargo.toml`, and every crate that uses `git2` inherits that entry.
//! Feature unification thus gives each of them the same `libgit2`, so one
//! crate is sufficient to hold the rules. `org-borg` holds them because it is
//! the crate that clones and fetches over SSH and over HTTPS.

/// `org-borg` clones and fetches over SSH with `Cred::ssh_key_from_agent` and
/// over HTTPS with `Cred::userpass_plaintext`. A `libgit2` built without one
/// of the two transports compiles clean and fails only at run time, on the
/// first repository that uses that transport.
#[test]
fn workspace_git2_keeps_the_ssh_and_https_transports() {
    let version = git2::Version::get();

    assert!(
        version.ssh(),
        "git2 links a libgit2 without SSH. Keep the default `ssh` feature of \
         the `git2` entry in [workspace.dependencies] of the root Cargo.toml."
    );
    assert!(
        version.https(),
        "git2 links a libgit2 without HTTPS. Keep the default `https` feature \
         of the `git2` entry in [workspace.dependencies] of the root Cargo.toml."
    );
}
