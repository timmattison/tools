//! Enforces issue #283's "safe by construction" guarantee: every integration
//! test that spawns the real `nwt` binary must go through `support::nwt_command`,
//! which scrubs `ZELLIJ`/`TMUX` from the child. A test that reaches for the raw
//! `CARGO_BIN_EXE_nwt` path bypasses that scrub and — when the suite is run from
//! inside a multiplexer — could hijack the user's tab. This guard fails if any
//! sibling test file references the raw binary path, so the bypass can't creep
//! back in.
//!
//! `support/mod.rs` (the shared builder) and this guard file are the sole
//! sanctioned homes of the raw path (see `SANCTIONED_TOKEN_HOMES`); the
//! recursive scan visits them but allowlists them, so the bypass stays caught
//! everywhere else in the `tests/` tree.

use std::fs;
use std::path::Path;

/// The token a test must NOT contain: cargo's env var naming the real nwt
/// binary. Only the shared builder in `support/mod.rs` may use it.
const RAW_SPAWN_TOKEN: &str = "CARGO_BIN_EXE_nwt";

/// Relative paths (forward-slash, relative to the scanned root) that are the
/// SANCTIONED homes of the raw `CARGO_BIN_EXE_nwt` token: the shared builder and
/// this guard file itself (which necessarily mentions the token in its consts
/// and mutation test). Everything else that references the token is an offender.
const SANCTIONED_TOKEN_HOMES: &[&str] = &["support/mod.rs", "no-raw-nwt-spawn.rs"];

/// Returns true if `source` reaches for the raw nwt binary path instead of going
/// through the shared builder.
fn references_raw_nwt_binary(source: &str) -> bool {
    source.contains(RAW_SPAWN_TOKEN)
}

/// Recursively scan `root` for `.rs` files that reference the raw nwt binary
/// path, returning the offenders' paths relative to `root` (forward-slash,
/// sorted), excluding the sanctioned homes in `SANCTIONED_TOKEN_HOMES`.
///
/// The scan descends into every subdirectory so a raw spawn hidden in a helper
/// module (e.g. `tests/<subdir>/foo.rs`) can't slip past the guard.
fn scan_for_raw_spawns(root: &Path) -> Vec<String> {
    let mut offenders = Vec::new();
    collect_raw_spawns(root, root, &mut offenders);
    offenders.sort();
    offenders
}

/// Recursive worker for [`scan_for_raw_spawns`]: walks `dir`, recording any
/// non-sanctioned `.rs` file under `root` that references the raw token. Paths
/// are recorded relative to `root` with forward slashes.
fn collect_raw_spawns(root: &Path, dir: &Path, offenders: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read nwt tests dir") {
        let path = entry.expect("read dir entry").path();

        if path.is_dir() {
            collect_raw_spawns(root, &path, offenders);
            continue;
        }

        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .expect("scanned path lies under root")
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if SANCTIONED_TOKEN_HOMES.contains(&rel.as_str()) {
            continue;
        }

        let source = fs::read_to_string(&path).expect("read test source");
        if references_raw_nwt_binary(&source) {
            offenders.push(rel);
        }
    }
}

#[test]
fn scanner_flags_a_planted_violation() {
    // Mutation check: prove the matcher actually fires on a real violation and
    // stays quiet on the sanctioned builder call. Without this, a broken matcher
    // could let `no_integration_test_spawns_the_raw_nwt_binary` pass vacuously.
    assert!(
        references_raw_nwt_binary("let c = Command::new(env!(\"CARGO_BIN_EXE_nwt\"));"),
        "the scanner must flag a direct CARGO_BIN_EXE_nwt reference"
    );
    assert!(
        !references_raw_nwt_binary("let c = support::nwt_command(&repo);"),
        "the scanner must not flag the sanctioned builder call"
    );
}

#[test]
fn scanner_recurses_into_test_subdirectories() {
    // A raw spawn can hide in a helper module pulled in from a subdirectory of
    // `tests/` (e.g. `tests/nested/sneaky.rs`). The scanner must descend into
    // those subdirectories, not just read top-level `.rs` files, or such a spawn
    // would bypass the ZELLIJ/TMUX scrub undetected.
    let temp = tempfile::TempDir::new().expect("create temp tests dir");
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).expect("create nested subdir");

    // Build the offending source without writing the bare token into this guard
    // file (which would make this file itself an unsanctioned offender).
    let sneaky_source = format!("let c = Command::new(env!(\"{RAW_SPAWN_TOKEN}\"));");
    fs::write(nested.join("sneaky.rs"), sneaky_source).expect("write sneaky test");

    let offenders = scan_for_raw_spawns(temp.path());

    assert!(
        offenders.contains(&"nested/sneaky.rs".to_string()),
        "the scanner must report raw spawns hidden in tests/ subdirectories, but \
         it reported {offenders:?}"
    );
}

#[test]
fn no_integration_test_spawns_the_raw_nwt_binary() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");

    let offenders = scan_for_raw_spawns(&tests_dir);

    assert!(
        offenders.is_empty(),
        "these integration tests spawn the raw nwt binary directly instead of \
         going through support::nwt_command, bypassing the ZELLIJ/TMUX scrub \
         (issue #283): {offenders:?}"
    );
}
