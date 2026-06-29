//! Enforces issue #283's "safe by construction" guarantee: every integration
//! test that spawns the real `nwt` binary must go through `support::nwt_command`,
//! which scrubs `ZELLIJ`/`TMUX` from the child. A test that reaches for the raw
//! `CARGO_BIN_EXE_nwt` path bypasses that scrub and — when the suite is run from
//! inside a multiplexer — could hijack the user's tab. This guard fails if any
//! sibling test file references the raw binary path, so the bypass can't creep
//! back in.
//!
//! `support/mod.rs` is the single sanctioned home of the raw path (it lives in a
//! subdirectory, so the top-level scan never visits it), and this guard skips
//! itself.

use std::fs;
use std::path::Path;

/// The token a test must NOT contain: cargo's env var naming the real nwt
/// binary. Only the shared builder in `support/mod.rs` may use it.
const RAW_SPAWN_TOKEN: &str = "CARGO_BIN_EXE_nwt";

/// This guard's own file name, skipped during the scan (it necessarily mentions
/// the forbidden token in this very const and in the mutation test below).
const SELF_FILE: &str = "no-raw-nwt-spawn.rs";

/// Returns true if `source` reaches for the raw nwt binary path instead of going
/// through the shared builder.
fn references_raw_nwt_binary(source: &str) -> bool {
    source.contains(RAW_SPAWN_TOKEN)
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
fn no_integration_test_spawns_the_raw_nwt_binary() {
    let tests_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");

    let mut offenders = Vec::new();
    for entry in fs::read_dir(&tests_dir).expect("read nwt tests dir") {
        let path = entry.expect("read dir entry").path();

        // Only top-level `.rs` test files. The `support/` subdir (home of the
        // sanctioned builder) is a directory and is skipped here.
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name == SELF_FILE {
            continue;
        }

        let source = fs::read_to_string(&path).expect("read test source");
        if references_raw_nwt_binary(&source) {
            offenders.push(name);
        }
    }
    offenders.sort();

    assert!(
        offenders.is_empty(),
        "these integration tests spawn the raw nwt binary directly instead of \
         going through support::nwt_command, bypassing the ZELLIJ/TMUX scrub \
         (issue #283): {offenders:?}"
    );
}
