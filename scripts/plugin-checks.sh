#!/usr/bin/env bash
# Run the one list of plugin checks for this repository.
#
# Two callers start this script, and no other file holds the list:
#
#   - .husky/pre-commit, when a staged change touches plugins/,
#     .claude-plugin/, or this script.
#   - .github/workflows/ci.yml, for each pull request and each push to main.
#
# Add a new check of a plugin to this script, not to a caller. Then the hook
# and CI cannot run different checks.
#
# The Claude Code plugins under plugins/ are TypeScript, and
# scripts/rust-checks.sh reads none of them. This script runs the checks of the
# claude CLI:
#
#   1. claude plugin validate .        - reads the marketplace file at the
#                                        root of the repository.
#   2. claude plugin validate <plugin> - reads the manifest and each hooks
#                                        module the way the engine loads them.
#   3. claude plugin test <plugin>     - runs the tests of the plugin in the
#                                        sandbox of the engine, which has no
#                                        file system, network or process.
#
# These checks need no login. A machine without the claude CLI cannot run
# them, and a run that skips them reports a plugin that it never read as
# clean. So that case fails, and the message names the fix.
#
# The script reads the working tree of the repository that holds it, whatever
# the current directory of the caller is.
#
# Each claude command runs through `without_git_environment`. Git exports GIT_
# variables into each hook, and every program that the hook starts inherits
# them. The helper removes the whole GIT_ prefix and never a list of names, so
# GIT_CONFIG_PARAMETERS and each name that git adds later stay out of the CLI.
set -euo pipefail

readonly LOG_PREFIX="plugin-checks:"

usage() {
    echo "Usage: $0 [-h|--help]"
    echo ""
    echo "Runs the checks of the Claude Code plugins of this repository:"
    echo "claude plugin validate and claude plugin test. The pre-commit hook"
    echo "and CI both run it."
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

if ! command -v claude >/dev/null 2>&1; then
    echo "$LOG_PREFIX the claude CLI is not on PATH, and it runs the checks of plugins/. Install Claude Code, then run the checks again." >&2
    exit 1
fi

echo "$LOG_PREFIX running claude plugin validate on the marketplace"
without_git_environment claude plugin validate .
for plugin in plugins/*/; do
    # A glob that matches no directory stays the text plugins/*/.
    [ -d "${plugin}" ] || continue
    echo "$LOG_PREFIX running claude plugin validate and claude plugin test on ${plugin}"
    without_git_environment claude plugin validate "${plugin}"
    without_git_environment claude plugin test "${plugin}"
done
