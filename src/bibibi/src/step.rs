//! One step of the search: the types of a commit and of its verdict.

use std::fmt;

/// A full object id that git resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit(pub String);

impl Commit {
    /// The first seven characters of the id.
    pub fn short(&self) -> String {
        self.0.chars().take(7).collect()
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one step decided about one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The test passed.
    Good,
    /// The test failed.
    Bad,
    /// The commit cannot be judged, for the stated reason.
    Skip(SkipReason),
}

/// Why a step skipped a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The patch of the test commit does not apply.
    PatchDoesNotApply,
    /// The build exited with this non-zero code.
    BuildExited(i32),
    /// This signal stopped the build.
    BuildKilled(i32),
    /// The test exited with code 125.
    TestExited125,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Stub: the green commit writes the texts.
        let _ = self;
        f.write_str("")
    }
}

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;

use anyhow::{bail, Result};
use gitscratch::Scratch;

use crate::child;

/// Test one commit: check it out, apply the patch, run the test, and restore
/// the scratch worktree. The restore happens on every path.
///
/// Exit code 0 is good. Any other code is bad.
///
/// # Errors
///
/// Returns an error if git fails, if the patch does not apply, if the test
/// cannot start, or if a signal ends the test.
pub fn step(
    scratch: &Scratch,
    patch: &Path,
    test_command: &[OsString],
    commit: &Commit,
) -> Result<Verdict> {
    let verdict = test_at(scratch, patch, test_command, commit);
    let restored = scratch.restore();
    let verdict = verdict?;
    restored?;
    Ok(verdict)
}

/// The steps before the restore.
fn test_at(
    scratch: &Scratch,
    patch: &Path,
    test_command: &[OsString],
    commit: &Commit,
) -> Result<Verdict> {
    scratch.check_out_detached(&commit.0)?;
    if !scratch.apply_patch(patch)? {
        bail!("the patch does not apply at {}", commit.short());
    }
    let status = child::run(test_command, scratch.path())?;
    if let Some(signal) = status.signal() {
        bail!("a signal ({signal}) stopped the test at {}", commit.short());
    }
    Ok(if status.success() {
        Verdict::Good
    } else {
        Verdict::Bad
    })
}
