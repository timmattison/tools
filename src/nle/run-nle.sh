#!/bin/bash
# Run the NLE (Non-Linear Editor) Tauri application in development mode
#
# Usage:
#   ./src/nle/run-nle.sh

set -e

cd "$(dirname "$0")"

# The Rust toolchain, the Tauri CLI and portplz are per-account installs, and
# this checkout is writable only by the account that owns it. On a shared machine
# the account at the keyboard is often neither of those, so name the account in
# every message.
user="$(id -un)"

if ! command -v cargo > /dev/null 2>&1; then
    echo "run-nle: cargo is not on the PATH of the account '$user'." >&2
    echo "  Rust installs per account under ~/.cargo, and one account cannot" >&2
    echo "  read another's. Run this from the account that owns the checkout," >&2
    echo "  or install Rust for '$user' from https://rustup.rs" >&2
    exit 1
fi

if ! cargo tauri --version > /dev/null 2>&1; then
    echo "run-nle: the Tauri CLI is not installed for the account '$user'." >&2
    echo "  Install it with:" >&2
    echo "    cargo install tauri-cli" >&2
    echo "  Or run this from the account that already has it." >&2
    exit 1
fi

if ! command -v portplz > /dev/null 2>&1; then
    echo "run-nle: portplz is not on the PATH of the account '$user'." >&2
    echo "  It picks the dev server port, so two branches, two worktrees or" >&2
    echo "  two accounts never collide. Install it from this repository:" >&2
    echo "    cargo install --path ../portplz" >&2
    exit 1
fi

if [ ! -w . ]; then
    echo "run-nle: the account '$user' cannot write to $(pwd)." >&2
    echo "  The build writes to target/ and frontend/node_modules/." >&2
    echo "  Run this from the account that owns the checkout." >&2
    exit 1
fi

# portplz derives the port from the repository, the branch and the user, so it is
# the same on every run and different in every worktree. Vite reads it from the
# environment and Tauri gets it through --config, so the two always agree.
port="$(portplz)"
if ! [[ "$port" =~ ^[0-9]+$ ]] || [ "$port" -lt 1 ] || [ "$port" -gt 65535 ]; then
    echo "run-nle: portplz gave '$port', which is not a port number." >&2
    exit 1
fi

export NLE_DEV_PORT="$port"
echo "Starting the dev server on port $port (chosen by portplz)"

# Ensure frontend dependencies are installed
if [ ! -d "frontend/node_modules" ]; then
    echo "Installing frontend dependencies..."
    pnpm --dir frontend install
fi

cargo tauri dev --config "{\"build\":{\"devUrl\":\"http://localhost:$port\"}}"
