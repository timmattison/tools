#!/usr/bin/env bash
# Run the one list of Rust checks for this repository.
#
# Two callers start this script, and no other file holds the list:
#
#   - .husky/pre-commit, when a staged change touches Rust.
#   - .github/workflows/ci.yml, for each pull request and each push to main.
#
# Add a new check to this script, not to a caller. Then the hook and CI cannot
# run different checks.
#
# The four Rust gates run cheapest-first so a run fails fast on the quickest
# check:
#
#   1. cargo fmt --check  - formatting drift; no compile, so it is fastest.
#   2. cargo machete      - unused-dependency detector. Like fmt it compiles
#                           nothing (it parses Cargo manifests and sources), so
#                           it runs before clippy. Plain mode is used (not
#                           --with-metadata) so it never touches Cargo.lock; the
#                           crates with known false positives carry
#                           [package.metadata.cargo-machete] ignored entries.
#                           A non-zero exit means an unused dependency slipped
#                           into a manifest, which fails the run.
#   3. cargo clippy       - lints as hard errors. --all-targets covers tests/
#                           and benches/, where lints like
#                           unseparated_literal_suffix would otherwise slip
#                           through, and -D warnings turns every workspace lint
#                           into a hard error.
#   4. cargo test         - the workspace test suite. Slowest, so it runs last;
#                           clippy's --all-targets has already compiled the test
#                           targets, so this pass is mostly incremental + run.
#
# The script reads the working tree of the repository that holds it, whatever
# the current directory of the caller is.
set -euo pipefail

readonly LOG_PREFIX="rust-checks:"

usage() {
    echo "Usage: $0 [-h|--help]"
    echo ""
    echo "Runs the Rust checks of this repository: cargo fmt, cargo machete,"
    echo "cargo clippy, and cargo test. The pre-commit hook and CI both run it."
    echo ""
    echo "Options:"
    echo "  -h, --help  Show this help message"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "$LOG_PREFIX unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

cd "$(dirname "$0")/.."

echo "$LOG_PREFIX running cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "$LOG_PREFIX running cargo machete (unused dependency check)"
if ! command -v cargo-machete >/dev/null 2>&1; then
    echo "$LOG_PREFIX cargo-machete not found; installing it once (cargo install cargo-machete --locked)…"
    cargo install cargo-machete --locked
fi
cargo machete

echo "$LOG_PREFIX running cargo clippy --all-targets --workspace -- -D warnings"
cargo clippy --all-targets --workspace -- -D warnings

echo "$LOG_PREFIX running cargo test --workspace (CLICOLOR_FORCE=1)"
# Git exports GIT_DIR/GIT_WORK_TREE/GIT_INDEX_FILE/GIT_PREFIX into the hook's
# environment, and `cargo test` inherits them. The suite spawns helpers that
# run `git init`/`add`/`commit` inside throwaway temp repos; if those vars
# leak through, the child git targets THIS repo instead of the temp dir and
# can rewrite the real index and branch. (Observed: a gsw test committed its
# `a.txt` fixture plus the developer's staged changes straight onto the
# branch, and parallel runs flooded the real .git/config with lock errors.)
# Scrub them for the test run so every test's git stays sandboxed.
#
# This is now defence in depth rather than the only defence, and it stays
# for two reasons. It is per-INVOCATION, so it protects nothing about the
# other routes to the same `cargo test` - a pre-push hook, `git bisect run`,
# `rebase --exec`, `swt`'s green check called from a hook - which is why
# `gitscratch` sheds every `GIT_`-prefixed variable, and not a list of
# names, at every git spawn of its own - the rule `NoInheritedGitEnvironment`
# applies, pinned by `src/gitscratch/tests/inherited-environment.rs`. That
# is the suite that pins the *prefix*: it probes GIT_CONFIG_PARAMETERS,
# GIT_CONFIG_COUNT and a name git has never invented, and it keeps a list of
# names that must survive, so the rule stays a prefix rather than a sweep of
# everything. `src/gitscratch/tests/isolation.rs` pins the separate claim
# that a decoy repository comes back byte-identical; it leaks only names a
# list would remove too, so it stays green with the rule narrowed back to a
# list - measured, and recorded in `src/gitscratch/MUTATIONS.md`.
# And it is workspace-wide, where that fix is not: every other crate's
# fixtures still lean on this line.
#
# CLICOLOR_FORCE=1 makes the `colored` crate paint whatever the run writes
# to. Without it, `colored` paints only when file descriptor 1 is a
# terminal, so a test that compares painted output against plain text
# passes in a redirected run and fails under a hand-typed commit, where the
# hook passes its terminal straight through. That is a deterministic result
# on a condition the test does not control, and it stays hidden until
# somebody commits from a terminal. Forcing the codes on makes the gate
# strictly stricter: such a test now fails on every commit. A test that
# asserts on rendered text must compare visible glyphs, through the
# `testcolor` crate — see "Colored Output in Tests" in CLAUDE.md.
#
# This does not defeat a tool that decides its own color output. A manual
# `colored::control::set_override` beats CLICOLOR_FORCE, so `gsw --no-color`
# and `seescc` under NO_COLOR still print plain text, and
# `testcolor::with_forced_ansi` still forces the codes on.
env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE -u GIT_PREFIX \
    CLICOLOR_FORCE=1 cargo test --workspace
