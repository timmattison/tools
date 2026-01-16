//! Build script that captures git information at compile time.
//!
//! Sets environment variables for use by the library:
//! - `BUILD_GIT_HASH`: Short git commit hash (7 chars) or "unknown"
//! - `BUILD_GIT_DIRTY`: "dirty", "clean", or "unknown"
//!
//! # Note on Directory Structure
//!
//! This crate assumes it lives at `src/buildinfo/` relative to the workspace root.
//! The `.git` directory paths are calculated relative to this location.

use std::path::Path;
use std::process::Command;

fn main() {
    // Get the directory containing this build script (src/buildinfo/)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_path = Path::new(&manifest_dir);

    // Calculate paths to .git directory (workspace root is two levels up)
    let git_head = manifest_path.join("../../.git/HEAD");
    let git_index = manifest_path.join("../../.git/index");

    // Tell Cargo to rerun this if .git/HEAD or .git/index changes
    // This ensures rebuilds when commits change or files are staged
    println!("cargo:rerun-if-changed={}", git_head.display());
    println!("cargo:rerun-if-changed={}", git_index.display());

    let git_hash = get_git_hash().unwrap_or_else(|| "unknown".to_string());
    let git_dirty = get_git_dirty().unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=BUILD_GIT_HASH={git_hash}");
    println!("cargo:rustc-env=BUILD_GIT_DIRTY={git_dirty}");
}

fn get_git_hash() -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn get_git_dirty() -> Option<String> {
    // Check for unstaged changes
    let unstaged = Command::new("git")
        .args(["diff", "--quiet"])
        .output()
        .ok()?;

    if !unstaged.status.success() {
        return Some("dirty".to_string());
    }

    // Also check for staged changes
    let staged = Command::new("git")
        .args(["diff", "--quiet", "--cached"])
        .output()
        .ok()?;

    if staged.status.success() {
        Some("clean".to_string())
    } else {
        Some("dirty".to_string())
    }
}
