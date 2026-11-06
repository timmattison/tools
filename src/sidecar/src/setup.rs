//! `sidecar setup`: clone each sidecar and link it into the worktree.
//!
//! The rules, for each name of the registry in order:
//!
//! 1. A missing main clone is cloned on `main`.
//! 2. A foreign directory at the place of the clone gets a warning and no link.
//!    A clone that did not end gets a warning that names `sidecar setup`, and no link.
//! 3. A missing link is made.
//! 4. A link to the main clone or to a worktree of it stays.
//! 5. A broken link is pointed at the main clone.
//! 6. A foreign thing at the place of the link gets a warning and stays.
//! 7. A clone that failed, with a usable directory at the place now, uses that directory.
//! 8. A clone that failed, with nothing usable, gets a warning.

use crate::clone::{ensure_clone, CloneOutcome};
use crate::host::Host;
use crate::link::{inspect_link, point_link, LinkState};
use crate::name::SidecarName;

/// The command that runs the setup by hand. Each warning names it.
pub const SETUP_COMMAND: &str = "sidecar setup";

/// The variable that a CI runner sets.
pub const CI_VARIABLE: &str = "CI";

/// What one run of the setup did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetupReport {
    /// One line for each clone and each link that the run made.
    pub actions: Vec<String>,
    /// One line for each problem. The run continues after each one.
    pub warnings: Vec<String>,
    /// The sidecars whose link is not correct at the end of the run.
    pub unlinked: Vec<SidecarName>,
}

/// Runs the rules for each name of the registry, or only for `only`.
#[must_use]
pub fn run_setup(host: &Host, only: Option<&SidecarName>) -> SetupReport {
    let mut report = SetupReport::default();
    for name in host.registry().sidecars() {
        if only.is_some_and(|wanted| wanted != name) {
            continue;
        }
        if !set_up_one(host, name, &mut report) {
            report.unlinked.push(name.clone());
        }
    }
    report
}

/// Runs the rules for one name. Returns true when the link is correct at the end.
fn set_up_one(host: &Host, name: &SidecarName, report: &mut SetupReport) -> bool {
    let main_clone = host.main_clone(name);
    let url = match host.sidecar_url(name) {
        Ok(url) => url,
        Err(error) => {
            report.warnings.push(format!("{error:#}"));
            return false;
        }
    };
    match ensure_clone(host.git(), &url, &main_clone) {
        CloneOutcome::Made => report
            .actions
            .push(format!("Cloned {url} into {}.", main_clone.display())),
        CloneOutcome::Present => {}
        CloneOutcome::Incomplete | CloneOutcome::Foreign { .. } | CloneOutcome::Failed { .. } => {
            return false;
        }
    }
    let link = host.link(name);
    let shown = host.link_shown(name);
    match inspect_link(host.git(), &link, &main_clone) {
        LinkState::Missing => match point_link(&link, &main_clone) {
            Ok(()) => {
                report
                    .actions
                    .push(format!("Linked {shown} to {}.", main_clone.display()));
                true
            }
            Err(_) => false,
        },
        _ => true,
    }
}

/// None when git ignores the link directory. Else the refusal text, which holds the
/// line for `.gitignore`.
#[must_use]
pub fn ignore_refusal(host: &Host) -> Option<String> {
    if host.ignores_link_dir() {
        return None;
    }
    Some(format!(
        "git does not ignore {}/, thus a link can enter a commit. Add this line to .gitignore, and run `{SETUP_COMMAND}` again:\n\n    {}",
        host.registry().link_dir().display(),
        host.ignore_line()
    ))
}

/// True when `CI` is set to a value other than empty, `0`, or `false`.
/// GitHub Actions sets `CI=true`.
#[must_use]
pub fn is_ci(value: Option<&str>) -> bool {
    !matches!(value, None | Some("" | "0" | "false"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ci_is_false_for_no_value_and_for_the_off_values() {
        for value in [None, Some(""), Some("0"), Some("false")] {
            assert!(!is_ci(value), "{value:?}");
        }
    }

    #[test]
    fn is_ci_is_true_for_the_on_values() {
        for value in [Some("true"), Some("1"), Some("yes")] {
            assert!(is_ci(value), "{value:?}");
        }
    }
}
