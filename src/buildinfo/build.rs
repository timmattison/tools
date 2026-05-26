//! Build script that captures git information at compile time.
//!
//! Sets environment variables for use by the library:
//! - `BUILD_GIT_HASH`: Short git commit hash (7 chars) or "unknown"
//! - `BUILD_GIT_DIRTY`: "dirty", "clean", or "unknown"
//!
//! The git directory is discovered dynamically using `git rev-parse --git-dir`,
//! which works correctly in both regular repositories and git worktrees.

use std::path::PathBuf;
use std::process::Command;

// Share the rerun-path selection logic with the library's test suite so it can
// be unit tested (a build script cannot be tested directly).
#[path = "src/rerun.rs"]
mod rerun;

fn main() {
    // Dynamically find the git directory (works in worktrees too)
    if let Some(git_dir) = get_git_dir() {
        // Branch refs live in the common dir, which differs from `git_dir` in a
        // linked worktree; fall back to `git_dir` when they coincide.
        let git_common_dir = get_git_common_dir().unwrap_or_else(|| git_dir.clone());
        let head_contents = std::fs::read_to_string(git_dir.join("HEAD")).unwrap_or_default();

        // Tell Cargo which git files to watch so a new commit (including a moved
        // branch) forces this script to rerun and recapture the hash.
        for path in rerun::rerun_if_changed_paths(&git_dir, &git_common_dir, &head_contents) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

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

fn get_git_dir() -> Option<PathBuf> {
    git_rev_parse(&["rev-parse", "--git-dir"])
}

fn get_git_common_dir() -> Option<PathBuf> {
    git_rev_parse(&["rev-parse", "--git-common-dir"])
}

fn git_rev_parse(args: &[&str]) -> Option<PathBuf> {
    let output = Command::new("git").args(args).output().ok()?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(PathBuf::from(path))
    } else {
        None
    }
}
