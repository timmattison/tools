#!/bin/bash
# Install all tools from the buffalo-tools workspace.
#
# Usage:
#   From a local clone:
#     ./scripts/install-from-local.sh
#
#   From GitHub (no clone needed):
#     curl -sSf https://raw.githubusercontent.com/timmattison/tools/main/scripts/install-from-local.sh | bash
#
#   Options:
#     --list         List all tools without installing

set -euo pipefail

REPO_URL="https://github.com/timmattison/tools"
LIST_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --list) LIST_ONLY=true; shift ;;
        -h|--help)
            # Print the header comment, from the first line under the title to
            # the line before the first line that is not a comment. The end of
            # the comment block ends the help, so a line added to that block
            # shows up in the help and truncates nothing.
            sed -n '3,${/^#/!q;p;}' "$0"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# Determine repo root — local clone or temp clone
find_or_clone_repo() {
    # Check if we're inside a clone already
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local candidate
    candidate="$(dirname "$script_dir")"

    if [[ -f "$candidate/Cargo.toml" ]] && grep -q '\[workspace\]' "$candidate/Cargo.toml" 2>/dev/null; then
        echo "$candidate"
        return
    fi

    # Not in a clone — fetch from GitHub
    local tmpdir
    tmpdir="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$tmpdir'" EXIT
    echo "Cloning $REPO_URL ..." >&2
    git clone --depth 1 "$REPO_URL" "$tmpdir" >&2
    echo "$tmpdir"
}

REPO_ROOT="$(find_or_clone_repo)"

# Discover Rust binary crates
rust_packages=()
for dir in "$REPO_ROOT"/src/*/; do
    if [[ -f "$dir/src/main.rs" ]]; then
        rust_packages+=("$(basename "$dir")")
    fi
done

if $LIST_ONLY; then
    echo "Rust tools (${#rust_packages[@]}):"
    printf '  %s\n' "${rust_packages[@]}"
    echo ""
    echo "Total: ${#rust_packages[@]} tools"
    exit 0
fi

# Install Rust tools
if ! command -v cargo &>/dev/null; then
    echo "Error: cargo not found. Install Rust: https://rustup.rs" >&2
    exit 1
fi

echo "Installing ${#rust_packages[@]} Rust tools..."

# Build -p flags for cargo install
pkg_flags=()
for pkg in "${rust_packages[@]}"; do
    pkg_flags+=("-p" "$pkg")
done

# Use --git for remote, --path for local
if [[ -d "$REPO_ROOT/.git" ]] && git -C "$REPO_ROOT" remote get-url origin &>/dev/null; then
    cargo install --git "$REPO_URL" --locked "${pkg_flags[@]}"
else
    # Local checkout without remote — install each from path
    for pkg in "${rust_packages[@]}"; do
        cargo install --path "$REPO_ROOT/src/$pkg" --locked
    done
fi

echo ""
echo "Done! ${#rust_packages[@]} tools installed."
