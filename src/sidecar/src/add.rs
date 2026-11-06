//! `sidecar add <name> [--yes]`: make a sidecar repository and register it.

use std::io::{BufRead, IsTerminal};
use std::path::Path;

use crate::clone::{inspect_clone, CloneState};
use crate::gh;
use crate::git::MAIN_BRANCH;
use crate::host::{Host, RegistryPolicy};
use crate::link::{inspect_link, LinkState};
use crate::name::SidecarName;
use crate::registry::{self, REGISTRY_FILE};
use crate::remote::{self, DATA_INFIX};
use crate::setup::run_setup;

/// The question that `sidecar add` asks on standard error.
const QUESTION: &str = "Do these steps? [y/N] ";

/// What the question about the plan decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    /// Do the steps.
    Proceed,
    /// Stop. There is no `--yes` and no terminal to ask.
    NeedsYes,
    /// Stop. The user did not answer yes.
    Declined,
}

/// The URL where a person gives the CI token read access to a repository.
pub const PAT_SETTINGS_URL: &str = "https://github.com/settings/personal-access-tokens";

/// One step of the plan. The plan holds only the steps that are not done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    MakeRepository,
    FirstCommit,
    Register,
    Ignore,
    Clone,
    Link,
}

/// What `sidecar add` found before it changed anything.
struct Facts {
    owner: String,
    repo: String,
    url: String,
    /// The steps that are not done, in the order that `sidecar add` does them.
    steps: Vec<Step>,
}

/// Asks GitHub and git about the sidecar repository. Returns whether the repository
/// exists and whether it has the branch `main`.
fn read_repository(
    host: &Host,
    owner: &str,
    repo: &str,
    url: &str,
) -> anyhow::Result<(bool, bool)> {
    match gh::repository_visibility(owner, repo)? {
        None => return Ok((false, false)),
        Some(gh::Visibility::Private) => {}
        Some(gh::Visibility::Other(word)) => anyhow::bail!(
            "The repository {owner}/{repo} is {word}. A sidecar must be private. \
             sidecar add changes nothing."
        ),
    }
    let heads = host.git().run(
        host.worktree_root(),
        [
            "ls-remote",
            "--heads",
            url,
            &format!("refs/heads/{MAIN_BRANCH}"),
        ],
    );
    if !heads.ok {
        anyhow::bail!("git cannot read {url}: {}", heads.stderr);
    }
    Ok((true, !heads.stdout.is_empty()))
}

/// Reads the state of the world. This function changes nothing.
fn read_facts(host: &Host, name: &SidecarName) -> anyhow::Result<Facts> {
    let owner = remote::owner(host.origin())
        .ok_or_else(|| anyhow::anyhow!("the origin {} names no owner", host.origin()))?;
    let repo = host.sidecar_repository(name);
    let url = host.sidecar_url(name)?;
    let (repository_exists, has_main) = read_repository(host, &owner, &repo, &url)?;
    let main_clone = host.main_clone(name);
    let clone_ready = inspect_clone(host.git(), &url, &main_clone) == CloneState::Ready;
    let link_ready = matches!(
        inspect_link(host.git(), &host.link(name), &main_clone),
        LinkState::MainClone | LinkState::Worktree { .. }
    );
    let todo = [
        (Step::MakeRepository, !repository_exists),
        (Step::FirstCommit, !has_main),
        (Step::Register, !host.registry().contains(name)),
        (Step::Ignore, !host.ignores_link_dir()),
        (Step::Clone, !clone_ready),
        (Step::Link, !link_ready),
    ];
    let steps = todo
        .into_iter()
        .filter_map(|(step, needed)| needed.then_some(step))
        .collect();
    Ok(Facts {
        owner,
        repo,
        url,
        steps,
    })
}

/// The line of the plan for `step`.
fn describe(step: Step, host: &Host, name: &SidecarName, facts: &Facts) -> String {
    match step {
        Step::MakeRepository => format!(
            "make the private repository {}/{} on GitHub",
            facts.owner, facts.repo
        ),
        Step::FirstCommit => {
            format!("push a first commit on {MAIN_BRANCH} with README.md and .gitattributes")
        }
        Step::Register => format!("add \"{name}\" to {REGISTRY_FILE}"),
        Step::Ignore => format!("add \"{}\" to .gitignore", host.ignore_line()),
        Step::Clone => format!(
            "clone {} into {}",
            facts.url,
            host.main_clone(name).display()
        ),
        Step::Link => format!(
            "link {} to {}",
            host.link_shown(name),
            host.main_clone(name).display()
        ),
    }
}

/// Runs `sidecar add` in the directory `cwd`. Returns true when the link of the new
/// sidecar is correct at the end.
///
/// # Errors
///
/// Returns an error when the name is not valid or when a step fails.
pub fn run(cwd: &Path, raw_name: &str, yes: bool) -> anyhow::Result<bool> {
    let name = SidecarName::parse(raw_name)?;
    let host = Host::discover(cwd, RegistryPolicy::DefaultWhenMissing)?;
    let facts = read_facts(&host, &name)?;
    println!("sidecar add will do these steps:");
    for step in &facts.steps {
        println!("  - {}", describe(*step, &host, &name, &facts));
    }
    let stdin = std::io::stdin();
    match confirm(yes, stdin.is_terminal(), &mut stdin.lock()) {
        Confirmation::Proceed => {}
        Confirmation::NeedsYes => {
            anyhow::bail!("sidecar add needs a yes. Run it in a terminal, or add --yes.")
        }
        Confirmation::Declined => {
            println!("sidecar add changes nothing.");
            return Ok(false);
        }
    }
    do_steps(&host, &name, &facts)
}

/// The text of `.gitattributes` in the first commit. It stops git from changing line
/// endings, so a file with `\r\n` keeps its bytes.
const GITATTRIBUTES_TEXT: &str = "* -text\n";

/// The text of `README.md` in the first commit.
fn readme_text(host: &Host, name: &SidecarName) -> String {
    let host_name = host.name();
    let link = host.link_shown(name);
    format!(
        "# {host_name}{DATA_INFIX}{name}\n\n\
         The data of the sidecar `{name}` of the repository `{host_name}`.\n\n\
         `sidecar` links this repository at `{link}` in each worktree of `{host_name}`. \
         Run `sidecar setup` in a worktree of `{host_name}` to clone and link it.\n\n\
         Keep each file below 100 MB, because GitHub refuses a larger file. \
         This repository does not use Git LFS.\n"
    )
}

/// Runs git in `cwd` and turns a failure into an error.
fn git_must(host: &Host, cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let result = host.git().run(cwd, args);
    if result.ok {
        Ok(())
    } else {
        anyhow::bail!("git {} failed: {}", args.join(" "), result.stderr)
    }
}

/// Makes the first commit in a scratch repository and pushes it to the sidecar.
fn push_first_commit(host: &Host, name: &SidecarName, url: &str) -> anyhow::Result<()> {
    let scratch = tempfile::tempdir()?;
    let dir = scratch.path();
    std::fs::write(dir.join("README.md"), readme_text(host, name))?;
    std::fs::write(dir.join(".gitattributes"), GITATTRIBUTES_TEXT)?;
    git_must(host, dir, &["init", "--quiet", "-b", MAIN_BRANCH])?;
    git_must(host, dir, &["add", "README.md", ".gitattributes"])?;
    let message = format!("Start the sidecar {name} of {}", host.name());
    git_must(host, dir, &["commit", "--quiet", "-m", &message])?;
    git_must(
        host,
        dir,
        &[
            "push",
            "--quiet",
            url,
            &format!("{MAIN_BRANCH}:refs/heads/{MAIN_BRANCH}"),
        ],
    )
}

/// Does the steps of the plan in order. Returns true when the new link is correct.
fn do_steps(host: &Host, name: &SidecarName, facts: &Facts) -> anyhow::Result<bool> {
    if facts.steps.contains(&Step::MakeRepository) {
        let login = gh::authenticated_user()?;
        let owner_is_user = facts.owner.eq_ignore_ascii_case(&login);
        gh::create_private_repository(&facts.owner, &facts.repo, owner_is_user)?;
    }
    if facts.steps.contains(&Step::FirstCommit) {
        push_first_commit(host, name, &facts.url)?;
    }
    if facts.steps.contains(&Step::Register) {
        registry::add_name_to_file(&host.worktree_root().join(REGISTRY_FILE), name)?;
    }
    let reloaded = Host::discover(host.worktree_root(), RegistryPolicy::DefaultWhenMissing)?;
    let report = run_setup(&reloaded, Some(name));
    for action in &report.actions {
        println!("{action}");
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(report.unlinked.is_empty())
}

/// Decides whether to do the steps of the plan.
///
/// With `yes`, it does not read. Without `yes` and without a terminal, it refuses.
/// On a terminal, it asks on standard error and reads one line from `input`.
/// The answer `y` or `yes`, in any case and with spaces around, means yes.
pub fn confirm(yes: bool, is_terminal: bool, input: &mut impl BufRead) -> Confirmation {
    if yes {
        return Confirmation::Proceed;
    }
    if !is_terminal {
        return Confirmation::NeedsYes;
    }
    eprint!("{QUESTION}");
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return Confirmation::Declined;
    }
    if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Confirmation::Proceed
    } else {
        Confirmation::Declined
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn answer(yes: bool, is_terminal: bool, text: &str) -> (Confirmation, u64) {
        let mut input = Cursor::new(text.as_bytes().to_vec());
        let decision = confirm(yes, is_terminal, &mut input);
        (decision, input.position())
    }

    #[test]
    fn the_flag_goes_on_without_a_read() {
        for is_terminal in [true, false] {
            assert_eq!(
                answer(true, is_terminal, "n\n"),
                (Confirmation::Proceed, 0),
                "terminal: {is_terminal}"
            );
        }
    }

    #[test]
    fn no_terminal_and_no_flag_refuses_without_a_read() {
        assert_eq!(answer(false, false, "y\n"), (Confirmation::NeedsYes, 0));
    }

    #[test]
    fn a_yes_answer_on_a_terminal_goes_on() {
        for text in ["y\n", "YES\n", " yes \n", "Y\r\n", "yes"] {
            assert_eq!(
                answer(false, true, text).0,
                Confirmation::Proceed,
                "{text:?}"
            );
        }
    }

    #[test]
    fn any_other_answer_on_a_terminal_stops() {
        for text in ["n\n", "\n", "", "maybe\n", "yess\n", "no\n"] {
            assert_eq!(
                answer(false, true, text).0,
                Confirmation::Declined,
                "{text:?}"
            );
        }
    }
}
