#!/bin/bash
# Build Go tools with version information injected via ldflags
#
# Usage:
#   ./scripts/build-go.sh           # Build all Go tools
#   ./scripts/build-go.sh bm dirc   # Build specific tools

set -e

cd "$(dirname "$0")/.."

# Get git info
GIT_HASH=$(git rev-parse --short=7 HEAD 2>/dev/null || echo "unknown")

# Check if repo is dirty (has uncommitted changes)
if git diff --quiet 2>/dev/null && git diff --quiet --cached 2>/dev/null; then
    GIT_DIRTY="clean"
else
    GIT_DIRTY="dirty"
fi

# Read version from central VERSION file (we're already in repo root from cd above)
VERSION_FILE="VERSION"
if [ -f "$VERSION_FILE" ]; then
    VERSION=$(cat "$VERSION_FILE" | tr -d '[:space:]')
else
    echo "Warning: VERSION file not found at $VERSION_FILE, using default"
    VERSION="0.1.0"
fi

# Build ldflags
LDFLAGS="-X github.com/timmattison/tools/internal/version.GitHash=${GIT_HASH}"
LDFLAGS="${LDFLAGS} -X github.com/timmattison/tools/internal/version.GitDirty=${GIT_DIRTY}"
LDFLAGS="${LDFLAGS} -X github.com/timmattison/tools/internal/version.Version=${VERSION}"

# Create bin directory
mkdir -p bin

# List of all Go tools
ALL_TOOLS="bm dirc localnext prgz procinfo subito symfix"

# Determine which tools to build
if [ $# -eq 0 ]; then
    TOOLS="$ALL_TOOLS"
else
    TOOLS="$@"
fi

echo "Building Go tools with version info:"
echo "  Git Hash: ${GIT_HASH}"
echo "  Git Dirty: ${GIT_DIRTY}"
echo "  Version: ${VERSION}"
echo ""

for tool in $TOOLS; do
    echo "Building ${tool}..."
    go build -ldflags "${LDFLAGS}" -o "bin/${tool}" "./cmd/${tool}"
done

echo ""
echo "Done! Binaries are in ./bin/"
