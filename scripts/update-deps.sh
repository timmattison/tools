#!/bin/bash
# Update all Rust dependencies to their latest versions
#
# Usage:
#   ./scripts/update-deps.sh              # Update all deps
#   ./scripts/update-deps.sh --lock-only  # Update Cargo.lock without changing Cargo.toml
#
# Prerequisites:
#   - cargo-edit: cargo install cargo-edit (for `cargo upgrade`)
#   - llvm (macOS): brew install llvm (for bindgen compatibility with latest SDK)

set -e

cd "$(dirname "$0")/.."

LOCK_ONLY=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --lock-only)
            LOCK_ONLY=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [--lock-only]"
            echo ""
            echo "Options:"
            echo "  --lock-only   Only update Cargo.lock (don't bump Cargo.toml constraints)"
            echo "  -h, --help    Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Run '$0 --help' for usage."
            exit 1
            ;;
    esac
done

echo "=== Updating Rust dependencies ==="

# On macOS, bindgen's bundled libclang may be too old for the latest SDK.
# Use Homebrew's llvm if available, and set the SDK sysroot for bindgen.
if [[ "$(uname)" == "Darwin" ]]; then
    LLVM_PREFIX="$(brew --prefix llvm 2>/dev/null || true)"
    if [[ -n "$LLVM_PREFIX" ]] && [[ -d "$LLVM_PREFIX/lib" ]]; then
        export LIBCLANG_PATH="$LLVM_PREFIX/lib"
    fi
    SDK_PATH="$(xcrun --sdk macosx --show-sdk-path 2>/dev/null || true)"
    if [[ -n "$SDK_PATH" ]]; then
        export BINDGEN_EXTRA_CLANG_ARGS="-isysroot $SDK_PATH"
    fi
fi

if [[ "$LOCK_ONLY" == true ]]; then
    echo "Updating Cargo.lock (within existing semver bounds)..."
    cargo update
else
    if ! command -v cargo-upgrade &>/dev/null; then
        echo "Error: cargo-edit is required for upgrading Cargo.toml constraints."
        echo "Install it with: cargo install cargo-edit"
        echo "Or run with --lock-only to just update Cargo.lock."
        exit 1
    fi

    echo "Upgrading workspace dependency constraints in Cargo.toml..."
    cargo upgrade

    echo "Updating Cargo.lock..."
    cargo update
fi

echo "Building Rust workspace..."
cargo build --release

echo "Running Rust tests..."
cargo test

echo "[PASS] Rust dependencies updated successfully."
echo ""

echo "=== All dependency updates complete ==="
echo ""
echo "Review changes with: git diff"
echo "Commit with: git add -A && git commit -m 'chore: update dependencies'"
