//! `sidecar add <name> [--yes]`: make a sidecar repository and register it.

use std::io::BufRead;
use std::path::Path;

use crate::clone::{inspect_clone, CloneState};
use crate::gh;
use crate::git::MAIN_BRANCH;
use crate::host::{Host, RegistryPolicy};
use crate::link::{inspect_link, LinkState};
use crate::name::SidecarName;
use crate::registry::REGISTRY_FILE;
use crate::remote;

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
    repository_exists: bool,
    has_main: bool,
    registered: bool,
    ignored: bool,
    clone_ready: bool,
    link_ready: bool,
}

impl Facts {
    /// The steps that are not done, in the order that `sidecar add` does them.
    fn plan(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        if !self.repository_exists {
            steps.push(Step::MakeRepository);
        }
        if !self.has_main {
            steps.push(Step::FirstCommit);
        }
        if !self.registered {
            steps.push(Step::Register);
        }
        if !self.ignored {
            steps.push(Step::Ignore);
        }
        if !self.clone_ready {
            steps.push(Step::Clone);
        }
        if !self.link_ready {
            steps.push(Step::Link);
        }
        steps
    }
}

/// Reads the state of the world. This function changes nothing.
fn read_facts(host: &Host, name: &SidecarName) -> anyhow::Result<Facts> {
    let owner = remote::owner(host.origin())
        .ok_or_else(|| anyhow::anyhow!("the origin {} names no owner", host.origin()))?;
    let repo = host.sidecar_repository(name);
    let url = host.sidecar_url(name)?;
    let (repository_exists, has_main) = match gh::repository_visibility(&owner, &repo)? {
        None => (false, false),
        Some(_) => {
            let heads = host.git().run(
                host.worktree_root(),
                [
                    "ls-remote",
                    "--heads",
                    &url,
                    &format!("refs/heads/{MAIN_BRANCH}"),
                ],
            );
            if !heads.ok {
                anyhow::bail!("git cannot read {url}: {}", heads.stderr);
            }
            (true, !heads.stdout.is_empty())
        }
    };
    let main_clone = host.main_clone(name);
    Ok(Facts {
        owner,
        repo,
        clone_ready: inspect_clone(host.git(), &url, &main_clone) == CloneState::Ready,
        link_ready: matches!(
            inspect_link(host.git(), &host.link(name), &main_clone),
            LinkState::MainClone | LinkState::Worktree { .. }
        ),
        url,
        repository_exists,
        has_main,
        registered: host.registry().contains(name),
        ignored: host.ignores_link_dir(),
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
    let steps = facts.plan();
    println!("sidecar add will do these steps:");
    for step in &steps {
        println!("  - {}", describe(*step, &host, &name, &facts));
    }
    let _ = yes;
    anyhow::bail!("the steps are not done yet")
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
