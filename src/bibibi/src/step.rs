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
    #[cfg_attr(not(test), expect(dead_code, reason = "the patch slice constructs it"))]
    PatchDoesNotApply,
    /// The build exited with this non-zero code.
    BuildExited(i32),
    /// This signal stopped the build.
    BuildKilled(i32),
    /// The test exited with code 125.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the test exit code slice constructs it")
    )]
    TestExited125,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PatchDoesNotApply => f.write_str("the patch does not apply"),
            Self::BuildExited(code) => write!(f, "the build exited {code}"),
            Self::BuildKilled(signal) => write!(f, "a signal ({signal}) stopped the build"),
            Self::TestExited125 => f.write_str("the test exited 125"),
        }
    }
}

use std::ffi::OsString;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;

use anyhow::{bail, Context, Result};
use gitscratch::Scratch;

use crate::child;

/// Test one commit: check it out, apply the patch, run the build when there
/// is one, run the test, and restore the scratch worktree. The restore
/// happens on every path.
///
/// A build that exits non-zero is a skip, and so is a build that a signal
/// stops. The test does not run after a skip. For the test, exit code 0 is
/// good and any other code is bad.
///
/// # Errors
///
/// Returns an error if git fails, if the patch does not apply, if the build
/// or the test cannot start, or if a signal ends the test.
pub fn step(
    scratch: &Scratch,
    patch: &Path,
    build: Option<&str>,
    test_command: &[OsString],
    commit: &Commit,
) -> Result<Verdict> {
    let verdict = test_at(scratch, patch, build, test_command, commit);
    let restored = scratch.restore();
    let verdict = verdict?;
    restored?;
    Ok(verdict)
}

/// The steps before the restore.
fn test_at(
    scratch: &Scratch,
    patch: &Path,
    build: Option<&str>,
    test_command: &[OsString],
    commit: &Commit,
) -> Result<Verdict> {
    scratch.check_out_detached(&commit.0)?;
    if !scratch.apply_patch(patch)? {
        bail!("the patch does not apply at {}", commit.short());
    }
    if let Some(build) = build {
        let status = child::run_build(build, scratch.path())?;
        if let Some(signal) = status.signal() {
            return Ok(Verdict::Skip(SkipReason::BuildKilled(signal)));
        }
        if let Some(code) = status.code().filter(|code| *code != 0) {
            return Ok(Verdict::Skip(SkipReason::BuildExited(code)));
        }
    }
    let status = child::run(test_command, scratch.path())?;
    test_verdict(status).with_context(|| format!("at {}", commit.short()))
}

/// Map the exit status of the test to a verdict.
///
/// # Errors
///
/// Returns an error if a signal stopped the test.
fn test_verdict(status: ExitStatus) -> Result<Verdict> {
    if let Some(signal) = status.signal() {
        bail!("a signal ({signal}) stopped the test");
    }
    Ok(if status.success() {
        Verdict::Good
    } else {
        Verdict::Bad
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    use super::{test_verdict, SkipReason, Verdict};

    /// The status of a process that exited with `code`.
    fn exited(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    #[test]
    fn exit_0_is_good() {
        assert_eq!(test_verdict(exited(0)).expect("verdict"), Verdict::Good);
    }

    #[test]
    fn exit_1_is_bad() {
        assert_eq!(test_verdict(exited(1)).expect("verdict"), Verdict::Bad);
    }

    #[test]
    fn exit_125_is_a_skip() {
        assert_eq!(
            test_verdict(exited(125)).expect("verdict"),
            Verdict::Skip(SkipReason::TestExited125)
        );
    }

    #[test]
    fn exit_124_from_timeout_is_bad() {
        assert_eq!(test_verdict(exited(124)).expect("verdict"), Verdict::Bad);
    }

    #[test]
    fn a_death_by_signal_9_is_bad() {
        assert_eq!(
            test_verdict(ExitStatus::from_raw(9)).expect("verdict"),
            Verdict::Bad
        );
    }
}
