//! The GitHub CLI `gh`, for `sidecar add` only.
//!
//! Every `gh` spawn sheds the inherited git environment and has no standard input.
//! `sidecar` asks `gh` for strings only, with `--jq`, thus a color setting of the
//! environment cannot paint the answer.

use std::process::{Command, Output, Stdio};

use anyhow::{anyhow, Context};

/// The text that `gh` prints on standard error when the repository does not exist.
const NOT_FOUND_TEXT: &str = "HTTP 404";
/// The visibility word of a private repository.
const PRIVATE_WORD: &str = "private";

/// The visibility of a repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Only people with access see the repository.
    Private,
    /// Any other visibility. The text is the word that GitHub gave.
    Other(String),
}

/// Runs `gh` with `args`.
fn gh(args: &[&str]) -> anyhow::Result<Output> {
    let mut command = Command::new("gh");
    gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
    command
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("sidecar add needs the GitHub CLI gh, and it cannot start gh")
}

/// The standard error of `output` as one trimmed line.
fn stderr_line(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Asks GitHub for the visibility of `<owner>/<repo>`. Gives `None` when the
/// repository does not exist.
///
/// # Errors
///
/// Returns an error when `gh` cannot start or when it fails for a reason other than a
/// missing repository.
pub fn repository_visibility(owner: &str, repo: &str) -> anyhow::Result<Option<Visibility>> {
    let path = format!("repos/{owner}/{repo}");
    let output = gh(&["api", &path, "--jq", ".visibility"])?;
    if output.status.success() {
        let word = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(Some(if word == PRIVATE_WORD {
            Visibility::Private
        } else {
            Visibility::Other(word)
        }));
    }
    let message = stderr_line(&output);
    if message.contains(NOT_FOUND_TEXT) {
        return Ok(None);
    }
    Err(anyhow!("gh api {path} failed: {message}"))
}
