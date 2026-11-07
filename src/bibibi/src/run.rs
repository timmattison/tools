//! The whole run: resolve, make the patch and the scratch worktree, search,
//! and print the result.

use anyhow::{Context, Result};
use gitscratch::Repo;
use tempfile::NamedTempFile;

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
/// Returns an error if git fails or if a step fails.
pub fn run(cli: &Cli) -> Result<Outcome> {
    use std::io::Write;

    let repo = Repo::open(&std::env::current_dir().context("no current directory")?)?;
    let test = Commit(repo.resolve(cli.test_commit.as_deref().unwrap_or("HEAD"))?);
    let mut bad = Commit(match &cli.bad {
        Some(bad) => repo.resolve(bad)?,
        None => repo.resolve(&format!("{test}^"))?,
    });
    let mut goods = cli
        .good
        .iter()
        .map(|good| repo.resolve(good).map(Commit))
        .collect::<Result<Vec<_>>>()?;
    let mut skipped: Vec<(Commit, SkipReason)> = Vec::new();

    let mut patch_file = NamedTempFile::new().context("could not make the patch file")?;
    patch_file
        .write_all(&repo.patch_of(&test.0)?)
        .and_then(|()| patch_file.flush())
        .context("could not write the patch file")?;

    let scratch = repo.scratch(&bad.0)?;

    let candidates = loop {
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
            Verdict::Bad => bad = next.clone(),
            Verdict::Skip(reason) => skipped.push((next.clone(), reason)),
        }

        let good_ids: Vec<&str> = goods.iter().map(|good| good.0.as_str()).collect();
        let steps_left = scratch.bisect_steps(&bad.0, &good_ids)?;
        let subject = repo.subject(&next.0)?;
        eprintln!(
            "{}",
            report::progress_line(&next.short(), verdict, &subject, steps_left)
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
