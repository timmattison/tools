#!/bin/bash
# Install all tools from the buffalo-tools workspace.
#
# Usage:
#   From a local clone:
#     ./scripts/install.sh
#
#   From GitHub (no clone needed):
#     curl -sSf https://raw.githubusercontent.com/timmattison/tools/master/scripts/install.sh | bash
#
#   Options:
#     --rust-only    Skip Go tools
#     --go-only      Skip Rust tools
#     --list         List all tools without installing

set -euo pipefail

REPO_URL="https://github.com/timmattison/tools"
RUST_ONLY=false
GO_ONLY=false
LIST_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --rust-only) RUST_ONLY=true; shift ;;
        --go-only) GO_ONLY=true; shift ;;
        --list) LIST_ONLY=true; shift ;;
        -h|--help)
            head -14 "$0" | tail -12
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

# Discover Go tools
go_packages=()
if [[ -d "$REPO_ROOT/cmd" ]]; then
    for dir in "$REPO_ROOT"/cmd/*/; do
        go_packages+=("$(basename "$dir")")
    done
fi

if $LIST_ONLY; then
    echo "Rust tools (${#rust_packages[@]}):"
    printf '  %s\n' "${rust_packages[@]}"
    echo ""
    echo "Go tools (${#go_packages[@]}):"
    printf '  %s\n' "${go_packages[@]}"
    echo ""
    echo "Total: $(( ${#rust_packages[@]} + ${#go_packages[@]} )) tools"
    exit 0
fi

# Install Rust tools
if ! $GO_ONLY; then
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

    echo "Installed ${#rust_packages[@]} Rust tools."
fi

# Install Go tools
if ! $RUST_ONLY; then
    if [[ ${#go_packages[@]} -gt 0 ]]; then
        if ! command -v go &>/dev/null; then
            echo "Skipping Go tools (go not found)." >&2
        else
            echo "Building ${#go_packages[@]} Go tools..."
            "$REPO_ROOT/scripts/build-go.sh"

            # Copy Go binaries to cargo bin dir (or GOBIN, or ~/bin)
            local_bin="${CARGO_HOME:-$HOME/.cargo}/bin"
            mkdir -p "$local_bin"
            for tool in "${go_packages[@]}"; do
                if [[ -f "$REPO_ROOT/bin/$tool" ]]; then
                    cp "$REPO_ROOT/bin/$tool" "$local_bin/$tool"
                fi
            done
            echo "Installed ${#go_packages[@]} Go tools to $local_bin."
        fi
    fi
fi

echo ""
echo "Done! $(( ${#rust_packages[@]} + ${#go_packages[@]} )) tools installed."
