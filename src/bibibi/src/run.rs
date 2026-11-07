//! The whole run: resolve, make the patch and the scratch worktree, search,
//! and print the result.

use anyhow::{Context, Result};
use gitscratch::Repo;
use tempfile::NamedTempFile;

use crate::report;
use crate::step::{step, Commit, Verdict};
use crate::Cli;

/// Search for the first bad commit and print it on standard output.
///
/// The test commit is the positional argument, or `HEAD` when it is absent.
/// The bad commit is `--bad`, or the parent of the test commit when `--bad` is
/// absent. Every `--good` commit bounds the search, in the order given.
///
/// # Errors
///
/// Returns an error if git fails, if a step fails, or if the search ends with
/// no commit to name.
pub fn run(cli: &Cli) -> Result<()> {
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

    let mut patch_file = NamedTempFile::new().context("could not make the patch file")?;
    patch_file
        .write_all(&repo.patch_of(&test.0)?)
        .and_then(|()| patch_file.flush())
        .context("could not write the patch file")?;

    let scratch = repo.scratch(&bad.0)?;

    loop {
        let good_ids: Vec<&str> = goods.iter().map(|good| good.0.as_str()).collect();
        let listed = scratch.bisect_all(&bad.0, &good_ids)?;
        let Some(next) = listed.into_iter().find(|id| *id != bad.0).map(Commit) else {
            break;
        };

        let verdict = step(&scratch, patch_file.path(), &cli.test_command, &next)?;
        match verdict {
            Verdict::Good => goods.push(next.clone()),
            Verdict::Bad => bad = next.clone(),
        }

        let good_ids: Vec<&str> = goods.iter().map(|good| good.0.as_str()).collect();
        let steps_left = scratch.bisect_steps(&bad.0, &good_ids)?;
        let subject = repo.subject(&next.0)?;
        eprintln!(
            "{}",
            report::progress_line(&next.short(), verdict, &subject, steps_left)
        );
    }

    println!("{}", report::result_line(&bad.0, &repo.subject(&bad.0)?));
    Ok(())
}
