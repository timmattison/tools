//! Guard: this repository's committed `.gitignore` ignores `.swt-check`.
//!
//! `.swt-check` is `swt`'s green-check escape hatch. `swt` resolves it from the
//! parent repo root and runs it in place of the checks it would otherwise
//! detect (`src/swt/src/green_check.rs`), so it is a per-developer file by
//! design. A committed one hands every developer — and every subagent merge
//! `swt` gates — somebody else's idea of green.
//!
//! The rule was already lost once. It arrived on the `swt-fixes` branch against
//! the TypeScript `swt`, and the Rust port deleted that whole tree without
//! carrying the `.gitignore` line across. Nothing failed, because the omission
//! is spelled as an *absence*.
//!
//! # Why the test reads the source of the rule
//!
//! `git check-ignore` is equally happy with a rule from the developer's own
//! `core.excludesFile` or from `.git/info/exclude`. Neither one is committed. A
//! test that accepted them would pass on the machine that already had the file
//! and fail for everybody else, which is the false green this guard exists to
//! prevent. So the assertion reads the *source* git names, not just the exit
//! status, and the host's global and system config is scrubbed out of the run.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The per-developer override file `swt` looks for at the parent repo root.
const OVERRIDE_FILE: &str = ".swt-check";

/// The committed ignore file that must carry the rule. `git check-ignore -v`
/// names its source in this exact spelling when the rule comes from the file at
/// the repository root.
const COMMITTED_IGNORE_FILE: &str = ".gitignore";

/// The git location variables git exports into a hook's environment. This
/// repository's own pre-commit hook runs `cargo test`, so a test that shells out
/// to git inherits them and answers for whatever repository they name rather
/// than for this checkout.
const INHERITED_GIT_ENV: [&str; 4] = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"];

/// Absolute, canonical path to the repository root, two levels above this
/// crate's manifest.
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&root)
        .unwrap_or_else(|e| panic!("cannot canonicalize repo root {}: {e}", root.display()))
}

/// Run `git` at the repository root with the host scrubbed out: no inherited
/// git location variables, and no global or system config.
///
/// The config matters as much as the location. `core.excludesFile` is a
/// per-developer setting, and a rule read from it would make this guard pass on
/// one machine only.
fn git_at_repo_root(args: &[&str]) -> Output {
    let mut command = Command::new("git");
    command.current_dir(repo_root()).args(args);
    for name in INHERITED_GIT_ENV {
        command.env_remove(name);
    }
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    command.env("GIT_CONFIG_SYSTEM", "/dev/null");
    command
        .output()
        .unwrap_or_else(|e| panic!("cannot run git at the repository root: {e}"))
}

#[test]
fn committed_gitignore_ignores_the_swt_check_override() {
    let output = git_at_repo_root(&["check-ignore", "--verbose", OVERRIDE_FILE]);

    // `git check-ignore` exits 0 when a rule matches, 1 when none does, and 128
    // when it could not reach a verdict at all. The last one is a refusal, not
    // a clean report, so it gets its own message.
    match output.status.code() {
        Some(0) => {}
        Some(1) => panic!(
            "`{OVERRIDE_FILE}` is not ignored by this repository.\n\n\
             It is `swt`'s per-developer green-check override, so a developer who \
             writes one sees it in `git status` and can commit it. Add this line to \
             `{COMMITTED_IGNORE_FILE}` at the repository root:\n\n    \
             {OVERRIDE_FILE}\n\n\
             (git also reports a path as not ignored when the path is already \
             tracked. If `{OVERRIDE_FILE}` was committed, remove it from the index \
             as well.)"
        ),
        _ => panic!(
            "`git check-ignore` failed rather than reaching a verdict: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ),
    }

    // `-v` prints `<source>:<line>:<pattern>` then a tab and the pathname.
    let verdict = String::from_utf8_lossy(&output.stdout);
    let source = verdict.split(':').next().unwrap_or_default();
    assert_eq!(
        source,
        COMMITTED_IGNORE_FILE,
        "`{OVERRIDE_FILE}` is ignored, but the rule comes from `{source}` rather \
         than from the committed `{COMMITTED_IGNORE_FILE}`. An uncommitted source \
         covers this developer and nobody else. Full verdict: {}",
        verdict.trim_end()
    );
}
