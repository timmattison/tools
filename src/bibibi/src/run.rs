//! The whole run: resolve, make the patch and the scratch worktree, search,
//! and print the result.

use anyhow::{bail, Context, Result};
use gitscratch::Repo;
use tempfile::NamedTempFile;

use crate::report;
use crate::step::{step, Commit, Verdict};
use crate::Cli;

/// Search for the first bad commit and print it on standard output.
///
/// The test commit is `HEAD`, the bad commit is its parent, and only the first
/// `--good` counts.
///
/// # Errors
///
/// Returns an error if git fails, if a step fails, or if the search ends with
/// no commit to name.
pub fn run(cli: &Cli) -> Result<()> {
    use std::io::Write;

    let repo = Repo::open(&std::env::current_dir().context("no current directory")?)?;
    let test = Commit(repo.resolve("HEAD")?);
    let mut bad = Commit(repo.resolve(&format!("{test}^"))?);
    let first_good = cli.good.first().context("no good commit was given")?;
    let mut goods = vec![Commit(repo.resolve(first_good)?)];

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

    if bad == goods[0] {
        bail!("the bad commit is a good commit");
    }
    println!("{}", report::result_line(&bad.0, &repo.subject(&bad.0)?));
    Ok(())
}
