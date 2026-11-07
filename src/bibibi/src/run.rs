//! The whole run: resolve, make the patch and the scratch worktree, search,
//! and print the result.

use anyhow::{bail, Context, Result};
use std::path::Path;

use gitscratch::{Repo, Scratch};
use tempfile::NamedTempFile;

use crate::interrupt;
use crate::report;
use crate::step::{step, Commit, SkipReason, Verdict};
use crate::Cli;

/// How a finished search ended.
#[derive(Debug)]
pub enum Outcome {
    /// One first bad commit. `run` printed it.
    Found,
    /// Skips leave more than one candidate for the first bad commit. `run`
    /// printed each of them, in the order of `git rev-list --bisect-all`.
    Ambiguous,
    /// This signal stopped the run. The patch file and the scratch worktree
    /// are gone, and `run` printed no verdict.
    Interrupted(i32),
}

/// Refuse a test commit that has no single parent or changes no file.
/// Return the patch of the test commit, which git made one time.
///
/// This runs before the patch file and the scratch worktree exist.
fn refuse_unusable_test_commit(repo: &Repo, test: &Commit) -> Result<Vec<u8>> {
    let short = test.short();
    match repo.parents(&test.0)?.len() {
        0 => bail!(
            "the test commit {short} is a root commit. It has no parent to take the change from."
        ),
        1 => {}
        _ => bail!("the test commit {short} is a merge commit. Give a commit that has one parent."),
    }
    let patch = repo.patch_of(&test.0)?;
    if patch.is_empty() {
        bail!("the test commit {short} changes no file.");
    }
    Ok(patch)
}

/// Refuse a good commit that is the bad commit, or that the bad commit does
/// not descend from. The goods are checked in the order given.
fn refuse_unusable_goods(repo: &Repo, goods: &[Commit], bad: &Commit) -> Result<()> {
    for good in goods {
        if good == bad {
            bail!(
                "the good commit {} is the same commit as the bad commit.",
                good.short()
            );
        }
        if !repo.is_ancestor(&good.0, &bad.0)? {
            bail!(
                "the good commit {} is not an ancestor of the bad commit {}.",
                good.short(),
                bad.short()
            );
        }
    }
    Ok(())
}

/// Test each good commit in the order given, then the bad commit, and refuse
/// the run if the ends do not behave. A progress line on standard error
/// names each commit that was tested.
///
/// # Errors
///
/// Returns the refusal if a good commit does not pass or the bad commit does
/// not fail. Returns an error if git fails or a step fails.
fn preflight(
    repo: &Repo,
    scratch: &Scratch,
    patch: &Path,
    cli: &Cli,
    goods: &[Commit],
    bad: &Commit,
    width: usize,
) -> Result<()> {
    for good in goods {
        interrupt::check()?;
        let verdict = preflight_step(repo, scratch, patch, cli, good, width)?;
        let why = match verdict {
            Verdict::Good => continue,
            Verdict::Bad(failure) => failure.to_string(),
            Verdict::Skip(reason) => reason.to_string(),
        };
        bail!(
            "the test does not pass at the good commit {}: {why}.",
            good.short()
        );
    }
    interrupt::check()?;
    let verdict = preflight_step(repo, scratch, patch, cli, bad, width)?;
    let why = match verdict {
        Verdict::Bad(_) => return Ok(()),
        Verdict::Good => "the test exited 0".to_string(),
        Verdict::Skip(reason) => reason.to_string(),
    };
    bail!(
        "the test does not fail at the bad commit {}: {why}.",
        bad.short()
    )
}

/// Run one step for the pre-flight and print its line, at most `width` columns wide.
fn preflight_step(
    repo: &Repo,
    scratch: &Scratch,
    patch: &Path,
    cli: &Cli,
    commit: &Commit,
    width: usize,
) -> Result<Verdict> {
    let verdict = step(
        scratch,
        patch,
        cli.build.as_deref(),
        &cli.test_command,
        commit,
    )?;
    let subject = repo.subject(&commit.0)?;
    eprintln!(
        "{}",
        report::preflight_line(width, &commit.short(), verdict, &subject)
    );
    Ok(verdict)
}

/// Search for the first bad commit and print the result on standard output.
///
/// The test commit is the positional argument, or `HEAD` when it is absent.
/// The bad commit is `--bad`, or the parent of the test commit when `--bad` is
/// absent. Every `--good` commit bounds the search, in the order given.
///
/// A skipped commit is never tested again. The search ends when every listed
/// commit except the bad commit is skipped. If the bad commit stands alone,
/// the result is that commit, one line `<id> <subject>`. Otherwise each
/// candidate gets one line, and a line on standard error counts them.
///
/// # Errors
///
/// Returns an error if git fails, if a step fails, or if the commits are
/// unusable: a root or merge test commit, a test commit that changes no file,
/// a good commit equal to the bad commit, or a good commit that is not an
/// ancestor of the bad commit.
///
/// A SIGINT or SIGTERM ends the run with [`Outcome::Interrupted`]. The running
/// child stops, and nothing else prints.
pub fn run(cli: &Cli) -> Result<Outcome> {
    interrupt::arm();
    match search(cli) {
        // A signal can break a git child or a step in many ways. Whatever the
        // error is, the signal is the cause.
        Err(_) if interrupt::received().is_some() => Ok(Outcome::Interrupted(
            interrupt::received().unwrap_or_default(),
        )),
        other => other,
    }
}

/// The body of [`run`]. The patch file and the scratch worktree drop when this
/// function returns, also with an error.
fn search(cli: &Cli) -> Result<Outcome> {
    use std::io::Write;

    let repo = Repo::open(&std::env::current_dir().context("no current directory")?)?;
    let test = Commit(repo.resolve(cli.test_commit.as_deref().unwrap_or("HEAD"))?);
    let patch = refuse_unusable_test_commit(&repo, &test)?;
    let mut bad = Commit(match &cli.bad {
        Some(bad) => repo.resolve(bad)?,
        None => repo.resolve(&format!("{test}^"))?,
    });
    let mut goods = cli
        .good
        .iter()
        .map(|good| repo.resolve(good).map(Commit))
        .collect::<Result<Vec<_>>>()?;
    refuse_unusable_goods(&repo, &goods, &bad)?;
    let mut skipped: Vec<(Commit, SkipReason)> = Vec::new();

    let mut patch_file = NamedTempFile::new().context("could not make the patch file")?;
    patch_file
        .write_all(&patch)
        .and_then(|()| patch_file.flush())
        .context("could not write the patch file")?;

    let scratch = repo.scratch(&bad.0)?;

    // The width is read one time. A progress line fits it, see `report`.
    let width = usize::from(termbar::TerminalWidth::get_or_default());
    preflight(&repo, &scratch, patch_file.path(), cli, &goods, &bad, width)?;

    let candidates = loop {
        interrupt::check()?;
        let good_ids: Vec<&str> = goods.iter().map(|good| good.0.as_str()).collect();
        let listed: Vec<Commit> = scratch
            .bisect_all(&bad.0, &good_ids)?
            .into_iter()
            .map(Commit)
            .collect();
        let next = listed
            .iter()
            .find(|id| **id != bad && !skipped.iter().any(|(done, _)| done == *id));
        let Some(next) = next.cloned() else {
            break listed
                .into_iter()
                .filter(|id| *id == bad || skipped.iter().any(|(done, _)| done == id))
                .collect::<Vec<_>>();
        };

        let verdict = step(
            &scratch,
            patch_file.path(),
            cli.build.as_deref(),
            &cli.test_command,
            &next,
        )?;
        match verdict {
            Verdict::Good => goods.push(next.clone()),
            Verdict::Bad(_) => bad = next.clone(),
            Verdict::Skip(reason) => skipped.push((next.clone(), reason)),
        }

        let good_ids: Vec<&str> = goods.iter().map(|good| good.0.as_str()).collect();
        let steps_left = scratch.bisect_steps(&bad.0, &good_ids)?;
        let subject = repo.subject(&next.0)?;
        eprintln!(
            "{}",
            report::progress_line(width, &next.short(), verdict, &subject, steps_left)
        );
    };

    if candidates.len() <= 1 {
        println!("{}", report::result_line(&bad.0, &repo.subject(&bad.0)?));
        return Ok(Outcome::Found);
    }
    for candidate in &candidates {
        let reason = skipped
            .iter()
            .find(|(done, _)| done == candidate)
            .map(|(_, reason)| *reason);
        let subject = repo.subject(&candidate.0)?;
        println!("{}", report::candidate_line(&candidate.0, &subject, reason));
    }
    eprintln!("{}", report::skips_leave_line(candidates.len()));
    Ok(Outcome::Ambiguous)
}
