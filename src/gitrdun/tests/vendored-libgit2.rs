//! Holds the workspace-wide rules for the `libgit2` that `git2` links.
//!
//! The rules live on the `git2` entry of `[workspace.dependencies]` in the
//! root `Cargo.toml`, and every crate that uses `git2` inherits that entry.
//! Feature unification thus gives each of them the same `libgit2`, so one
//! crate is sufficient to hold the rules. `gitrdun` holds them because its
//! link is the one that failed on each Homebrew upgrade of `libgit2`.

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

/// No crate in the workspace clones, fetches, or pushes with `git2`. Each one
/// reads a repository on the local disk. Thus `git2` builds neither network
/// transport.
///
/// The `ssh` feature builds `libssh2-sys`, which links the Homebrew OpenSSL
/// dylib on a Mac. A crate that does not use SSH thus loads a Homebrew dylib
/// at run time for nothing. If a crate starts to clone or fetch, turn on only
/// the transport it uses, and change this test in the same commit.
#[test]
fn workspace_git2_builds_no_network_transport() {
    let version = git2::Version::get();

    assert!(
        !version.ssh(),
        "git2 links a libgit2 with SSH, and no crate in the workspace uses it. \
         Keep `default-features = false` on the `git2` entry in \
         [workspace.dependencies] of the root Cargo.toml, and do not turn on \
         its `ssh` feature."
    );
    assert!(
        !version.https(),
        "git2 links a libgit2 with HTTPS, and no crate in the workspace uses it. \
         Keep `default-features = false` on the `git2` entry in \
         [workspace.dependencies] of the root Cargo.toml, and do not turn on \
         its `https` feature."
    );
}
