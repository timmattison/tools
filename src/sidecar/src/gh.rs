//! The GitHub CLI `gh`, for `sidecar add` only.
//!
//! Every `gh` spawn sheds the inherited git environment and has no standard input.
//! `sidecar` asks `gh` for strings only, with `--jq`, thus a color setting of the
//! environment cannot paint the answer.

use std::process::{Output, Stdio};

use anyhow::{anyhow, bail, Context};

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
    let mut command = childenv::command("gh");
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

/// Asks GitHub for the login of the authenticated user.
///
/// # Errors
///
/// Returns an error when `gh` cannot start, when it fails, or when it gives no login.
pub fn authenticated_user() -> anyhow::Result<String> {
    let output = gh(&["api", "user", "--jq", ".login"])?;
    if !output.status.success() {
        bail!("gh api user failed: {}", stderr_line(&output));
    }
    let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if login.is_empty() {
        bail!("gh api user gave no login");
    }
    Ok(login)
}

/// Makes the private repository `<owner>/<repo>`. The repository belongs to the
/// authenticated user when `owner_is_user` is true, and to the organization `owner`
/// otherwise.
///
/// # Errors
///
/// Returns an error when `gh` cannot start or when GitHub refuses the new repository.
pub fn create_private_repository(
    owner: &str,
    repo: &str,
    owner_is_user: bool,
) -> anyhow::Result<()> {
    let path = if owner_is_user {
        "user/repos".to_string()
    } else {
        format!("orgs/{owner}/repos")
    };
    let name = format!("name={repo}");
    let output = gh(&[
        "api",
        "-X",
        "POST",
        &path,
        "-f",
        &name,
        "-F",
        "private=true",
        "--jq",
        ".full_name",
    ])?;
    if output.status.success() {
        return Ok(());
    }
    Err(anyhow!(
        "gh cannot make the repository {owner}/{repo}: {}",
        stderr_line(&output)
    ))
}
