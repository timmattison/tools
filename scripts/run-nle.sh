#!/bin/bash
# Run the NLE (Non-Linear Editor) Tauri application in development mode
#
# Usage:
#   ./scripts/run-nle.sh

set -e

cd "$(dirname "$0")/../src/nle"

# Ensure frontend dependencies are installed
if [ ! -d "frontend/node_modules" ]; then
    echo "Installing frontend dependencies..."
    pnpm --dir frontend install
fi

cargo tauri dev
