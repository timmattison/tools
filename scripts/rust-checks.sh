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
# CLAUDE.md, "Checks in an issue-fleet Step", gives what a step runs instead.
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
#                           into a hard error. --locked makes a Cargo.lock that
#                           does not match the manifests fail the run, and
#                           cargo does not rewrite it. Without --locked, CI
#                           rewrites the lock file in its checkout and passes,
#                           and then scripts/install-from-local.sh, which
#                           installs with --locked, fails on main.
#   4. cargo test         - the workspace test suite. Slowest, so it runs last;
#                           clippy's --all-targets has already compiled the test
#                           targets, so this pass is mostly incremental + run.
#                           It also uses --locked, for the same reason.
#                           --no-fail-fast runs each test target also after a
#                           target fails, so one run reports each failed target.
#                           Without it, cargo stops at the first failed target,
#                           and each later failure costs one more full run. The
#                           step still exits non-zero when a test fails.
#
# The script reads the working tree of the repository that holds it, whatever
# the current directory of the caller is.
#
# Every cargo command runs through `without_git_environment`. Git exports GIT_
# variables into each hook, and every program that the hook starts inherits
# them. A test that runs `git init` in a temp repo then aims at THIS repo and
# can rewrite its index and branch. The helper removes the whole GIT_ prefix
# and never a list of names. The scrub is in this script and not in a caller,
# so each route to the checks gets it: the pre-commit hook, CI, a pre-push
# hook, `git rebase --exec`, and `git bisect run`.
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

# shellcheck source=scripts/without-git-environment.sh
. scripts/without-git-environment.sh

echo "$LOG_PREFIX running cargo fmt --all -- --check"
without_git_environment cargo fmt --all -- --check

echo "$LOG_PREFIX running cargo machete (unused dependency check)"
if ! command -v cargo-machete >/dev/null 2>&1; then
    echo "$LOG_PREFIX cargo-machete not found; installing it once (cargo install cargo-machete --locked)…"
    without_git_environment cargo install cargo-machete --locked
fi
without_git_environment cargo machete

echo "$LOG_PREFIX running cargo clippy --locked --all-targets --workspace -- -D warnings"
without_git_environment cargo clippy --locked --all-targets --workspace -- -D warnings

echo "$LOG_PREFIX running cargo test --locked --no-fail-fast --workspace (CLICOLOR_FORCE=1)"
# Git exports GIT_ variables into the hook, and every program that the hook
# starts inherits them. Rust spawns of this workspace shed the same variables
# through the crate `childenv`, and clippy makes its constructors mandatory.
# The runner is the only layer for code that other crates own, such as the
# build script of a dependency.
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
#
# COLORTERM=truecolor holds the other input of `colored` that comes from the
# environment. Each time `colored` formats a 24-bit color, it reads COLORTERM.
# If the value is not `truecolor` or `24bit`, it writes the nearest of the 16
# basic colors. `colored` gives no API to override this read. Thus a test that
# asserts on a 24-bit color passes in a terminal that sets COLORTERM, and fails
# in CI and in each terminal that does not set it. The first CI run found 8
# such tests in `gsw`.
without_git_environment env CLICOLOR_FORCE=1 COLORTERM=truecolor \
    cargo test --locked --no-fail-fast --workspace
