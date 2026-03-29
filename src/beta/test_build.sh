#!/bin/bash
cd "$(dirname "$0")"
cargo build 2>&1 | head -100