//! One step of the search. [`step`] tests one commit. The module also holds the types of a commit and of its verdict.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;

use anyhow::Result;
use gitscratch::Scratch;

use crate::child::{self, Ended};
use crate::interrupt;

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
    /// The test failed, in the stated way.
    Bad(TestFailure),
    /// The commit cannot be judged, for the stated reason.
    Skip(SkipReason),
}

/// How a test failed. The pre-flight names it in its refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestFailure {
    /// The test exited with this code, which is neither 0 nor 125.
    Exited(i32),
    /// This signal stopped the test.
    Killed(i32),
}

impl fmt::Display for TestFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "the test exited {code}"),
            Self::Killed(signal) => write!(f, "a signal ({signal}) stopped the test"),
        }
    }
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
        match self {
            Self::PatchDoesNotApply => f.write_str("the patch does not apply"),
            Self::BuildExited(code) => write!(f, "the build exited {code}"),
            Self::BuildKilled(signal) => write!(f, "a signal ({signal}) stopped the build"),
            Self::TestExited125 => f.write_str("the test exited 125"),
        }
    }
}

/// The exit code of the test that means skip, as `git bisect run` defines it.
const TEST_SKIP_CODE: i32 = 125;

/// Test one commit: check it out, apply the patch, run the build when there
/// is one, run the test, and restore the scratch worktree. The restore
/// happens on every path.
///
/// The patch goes on with a plain apply first. When git refuses it, a
/// three-way apply gets a second try. When that fails too, the commit is a
/// skip ([`SkipReason::PatchDoesNotApply`]), and neither the build nor the
/// test runs. A build that exits non-zero is a skip, and so is a build that a signal
/// stops. The test does not run after a skip. For the test, see
/// [`test_verdict`]: exit code 0 is good, 125 is a skip, and any other code or
/// a signal is bad.
///
/// # Errors
///
/// Returns an error if git fails, if the build or the test cannot start, or if the verdict of the test cannot be made.
/// Returns [`interrupt::Interrupted`] if a signal arrived.
pub fn step(
    scratch: &Scratch,
    patch: &Path,
    build: Option<&str>,
    test_command: &[OsString],
    commit: &Commit,
) -> Result<Verdict> {
    interrupt::check()?;
    let verdict = test_at(scratch, patch, build, test_command, commit);
    // After a signal the scratch drop cleans up, and a restore would only run
    // git against a run that is over.
    interrupt::check()?;
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
    if !scratch.apply_patch(patch)? && !scratch.apply_patch_three_way(patch)? {
        return Ok(Verdict::Skip(SkipReason::PatchDoesNotApply));
    }
    if let Some(build) = build {
        match child::run_build(build, scratch.path())? {
            Ended::Killed(signal) => return Ok(Verdict::Skip(SkipReason::BuildKilled(signal))),
            Ended::Exited(0) => {}
            Ended::Exited(code) => return Ok(Verdict::Skip(SkipReason::BuildExited(code))),
        }
    }
    let ended = child::run(test_command, scratch.path())?;
    Ok(test_verdict(ended))
}

/// Map the end of the test to a verdict.
///
/// | Result of the test | Verdict |
/// | --- | --- |
/// | exit code 0 | good |
/// | exit code 125 | skip ([`SkipReason::TestExited125`]) |
/// | any other exit code (`timeout` gives 124) | bad ([`TestFailure::Exited`]) |
/// | a signal stops the test | bad ([`TestFailure::Killed`]) |
fn test_verdict(ended: Ended) -> Verdict {
    match ended {
        Ended::Killed(signal) => Verdict::Bad(TestFailure::Killed(signal)),
        Ended::Exited(0) => Verdict::Good,
        Ended::Exited(TEST_SKIP_CODE) => Verdict::Skip(SkipReason::TestExited125),
        Ended::Exited(code) => Verdict::Bad(TestFailure::Exited(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::{test_verdict, Ended, SkipReason, TestFailure, Verdict};

    /// The end of a process that exited with `code`.
    fn exited(code: i32) -> Ended {
        Ended::Exited(code)
    }

    #[test]
    fn exit_0_is_good() {
        assert_eq!(test_verdict(exited(0)), Verdict::Good);
    }

    #[test]
    fn exit_1_is_bad() {
        assert_eq!(
            test_verdict(exited(1)),
            Verdict::Bad(TestFailure::Exited(1))
        );
    }

    #[test]
    fn exit_125_is_a_skip() {
        assert_eq!(
            test_verdict(exited(125)),
            Verdict::Skip(SkipReason::TestExited125)
        );
    }

    #[test]
    fn exit_124_from_timeout_is_bad() {
        assert_eq!(
            test_verdict(exited(124)),
            Verdict::Bad(TestFailure::Exited(124))
        );
    }

    #[test]
    fn a_death_by_signal_9_is_bad() {
        assert_eq!(
            test_verdict(Ended::Killed(9)),
            Verdict::Bad(TestFailure::Killed(9))
        );
    }
}
