//! Holds the workspace-wide rules for the `libgit2` that `git2` links.
//!
//! The rules live on the `git2` entry of `[workspace.dependencies]` in the
//! root `Cargo.toml`, and every crate that uses `git2` inherits that entry.
//! Feature unification thus gives each of them the same `libgit2`, so one
//! crate is sufficient to hold the rules. `org-borg` holds them because it is
//! the crate that clones and fetches over SSH and over HTTPS.

/// `git2` must link the `libgit2` that `libgit2-sys` compiles from its bundled
/// source, never the `libgit2` of the system. `Version::vendored` is `true`
/// only for the bundled, statically linked build.
///
/// A system `libgit2` is a bad choice on a Mac with Homebrew. The build script
/// of `libgit2-sys` caches the version-pinned Cellar path that `pkg-config`
/// gives, and cargo does not run the script again when Homebrew replaces that
/// directory. Each Homebrew upgrade of `libgit2` thus stopped the link with
/// `ld: library 'git2' not found` until somebody cleaned `libgit2-sys`. The
/// binaries that `cargo install` put on the machine also loaded the Homebrew
/// dylib at run time, so a move to a new minor version broke each of them.
///
/// This test fails when `LIBGIT2_NO_VENDOR=1` is set, because that variable
/// forces the system `libgit2` even with the bundled source turned on. That
/// failure is correct: the build no longer obeys the rule.
#[test]
fn workspace_git2_links_the_bundled_libgit2_not_the_system_one() {
    let version = git2::Version::get();
    let (major, minor, patch) = version.libgit2_version();

    assert!(
        version.vendored(),
        "git2 links the system libgit2 {major}.{minor}.{patch}, not the bundled \
         source. Turn on the `vendored-libgit2` feature of the `git2` entry in \
         [workspace.dependencies] of the root Cargo.toml, and make sure that \
         LIBGIT2_NO_VENDOR is not set."
    );
}

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
