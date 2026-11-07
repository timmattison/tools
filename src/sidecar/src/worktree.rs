//! `sidecar worktree`: point the link of a sidecar at a worktree of its main clone.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

use crate::clone::{inspect_clone, CloneState};
use crate::git::MAIN_BRANCH;
use crate::host::Host;
use crate::link::{inspect_link, point_link, ForeignLink, LinkState};
use crate::name::SidecarName;
use crate::setup::SETUP_COMMAND;

/// What `point` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pointed {
    /// The sidecar.
    pub name: SidecarName,
    /// The branch that the link now reads.
    pub branch: String,
    /// The path that the link now points at.
    pub target: PathBuf,
    /// True when this run made the worktree.
    pub made_worktree: bool,
    /// One line for each problem that did not stop the run.
    pub warnings: Vec<String>,
}

/// Points the link of the sidecar `raw_name` at the worktree of `branch`. Without a
/// branch, the current branch of the host worktree is the branch.
///
/// # Errors
///
/// Returns an error that names the fix when a check fails.
pub fn point(host: &Host, raw_name: &str, branch: Option<&str>) -> anyhow::Result<Pointed> {
    let name = registered_name(host, raw_name)?;
    let main_clone = host.main_clone(&name);
    require_ready_clone(host, &name, &main_clone)?;
    let branch = match branch {
        Some(branch) => branch.to_string(),
        None => current_branch(host, &name)?,
    };
    let checked = host
        .git()
        .run(&main_clone, ["check-ref-format", "--branch", &branch]);
    if !checked.ok {
        bail!(
            "git refuses the branch name \"{branch}\": {}",
            checked.stderr
        );
    }
    require_replaceable_link(host, &name, &main_clone)?;
    let mut warnings = Vec::new();
    if branch == MAIN_BRANCH {
        point_link(&host.link(&name), &main_clone)
            .with_context(|| format!("cannot change {}", host.link_shown(&name)))?;
        return Ok(Pointed {
            name,
            branch,
            target: main_clone,
            made_worktree: false,
            warnings,
        });
    }
    if let Some(record) = worktree_on_branch(host, &main_clone, &branch)? {
        require_live_worktree(&record, &branch, &main_clone)?;
        let existing = record.path;
        point_link(&host.link(&name), &existing)
            .with_context(|| format!("cannot change {}", host.link_shown(&name)))?;
        return Ok(Pointed {
            name,
            branch,
            target: existing,
            made_worktree: false,
            warnings,
        });
    }
    let target = host.worktrees_dir(&name).join(&branch);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::symlink_metadata(&target).is_ok() {
        bail!(
            "{} already exists. sidecar does not touch it. Move it away, or name another branch.",
            target.display()
        );
    }
    let path = target.to_string_lossy();
    let local_ref = format!("refs/heads/{branch}");
    if ref_exists(host, &main_clone, &local_ref) {
        add_worktree(host, &main_clone, &[&path, &branch])?;
    } else {
        let fetched = host.git().run(&main_clone, ["fetch", "--quiet", "origin"]);
        if !fetched.ok {
            warnings.push(format!(
                "The fetch of origin in {} failed: {}. sidecar continues with the local branches.",
                main_clone.display(),
                fetched.stderr
            ));
        }
        let remote = format!("origin/{branch}");
        if ref_exists(host, &main_clone, &format!("refs/remotes/{remote}")) {
            add_worktree(
                host,
                &main_clone,
                &["--track", "-b", &branch, &path, &remote],
            )?;
        } else {
            let base = format!("refs/heads/{MAIN_BRANCH}");
            add_worktree(host, &main_clone, &["-b", &branch, &path, &base])?;
        }
    }
    point_link(&host.link(&name), &target)
        .with_context(|| format!("cannot change {}", host.link_shown(&name)))?;
    Ok(Pointed {
        name,
        branch,
        target,
        made_worktree: true,
        warnings,
    })
}

/// Parses `raw_name` and checks that the registry names it.
fn registered_name(host: &Host, raw_name: &str) -> anyhow::Result<SidecarName> {
    let registered = SidecarName::parse(raw_name)
        .ok()
        .filter(|name| host.registry().contains(name));
    if let Some(name) = registered {
        return Ok(name);
    }
    let names: Vec<String> = host
        .registry()
        .sidecars()
        .iter()
        .map(ToString::to_string)
        .collect();
    if names.is_empty() {
        bail!("sidecar.toml names no sidecar. Run `sidecar add {raw_name}`.");
    }
    bail!(
        "sidecar.toml does not name the sidecar \"{raw_name}\". The registered names are: {}.",
        names.join(", ")
    )
}

/// Checks that the main clone of `name` is complete.
fn require_ready_clone(host: &Host, name: &SidecarName, main_clone: &Path) -> anyhow::Result<()> {
    let url = host.sidecar_url(name)?;
    match inspect_clone(host.git(), &url, main_clone) {
        CloneState::Ready => Ok(()),
        CloneState::Missing => bail!(
            "The main clone {} is missing. Run `{SETUP_COMMAND}`.",
            main_clone.display()
        ),
        CloneState::Incomplete => bail!(
            "The main clone {} did not end. Run `{SETUP_COMMAND}`.",
            main_clone.display()
        ),
        CloneState::Foreign { reason } => bail!(
            "{} is not the main clone of {name}: {reason} Move it away, and run `{SETUP_COMMAND}`.",
            main_clone.display()
        ),
    }
}

/// Checks that the place of the link holds nothing that sidecar must keep.
fn require_replaceable_link(
    host: &Host,
    name: &SidecarName,
    main_clone: &Path,
) -> anyhow::Result<()> {
    let shown = host.link_shown(name);
    match inspect_link(host.git(), &host.link(name), main_clone) {
        LinkState::Foreign(ForeignLink::Directory) => bail!(
            "{shown} is a real directory. sidecar does not touch it. Move it away, and run the command again."
        ),
        LinkState::Foreign(ForeignLink::File) => bail!(
            "{shown} is a real file. sidecar does not touch it. Move it away, and run the command again."
        ),
        LinkState::Foreign(ForeignLink::OtherTarget(target)) => bail!(
            "{shown} points at {}, which is neither {} nor a worktree of it. sidecar does not touch it.",
            target.display(),
            main_clone.display()
        ),
        LinkState::Missing
        | LinkState::Broken { .. }
        | LinkState::MainClone
        | LinkState::Worktree { .. } => Ok(()),
    }
}

/// The current branch of the host worktree.
fn current_branch(host: &Host, name: &SidecarName) -> anyhow::Result<String> {
    let head = host.git().run(
        host.worktree_root(),
        ["symbolic-ref", "--quiet", "--short", "HEAD"],
    );
    if head.ok {
        Ok(head.stdout)
    } else {
        bail!(
            "The worktree {} has a detached HEAD. Name a branch: `sidecar worktree {name} <branch>`.",
            host.worktree_root().display()
        )
    }
}

/// Runs `git worktree add --quiet` in the main clone with `args`.
fn add_worktree(host: &Host, main_clone: &Path, args: &[&str]) -> anyhow::Result<()> {
    let mut full = vec!["worktree", "add", "--quiet"];
    full.extend_from_slice(args);
    let added = host.git().run(main_clone, full);
    if added.ok {
        Ok(())
    } else {
        bail!("git cannot make the worktree: {}", added.stderr)
    }
}

/// One record of `git worktree list --porcelain -z`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreeRecord {
    /// The path of the worktree.
    path: PathBuf,
    /// The branch that the worktree has checked out, if any.
    branch: Option<String>,
    /// True when the record has a `locked` field.
    locked: bool,
    /// True when the record has a `prunable` field.
    prunable: bool,
}

/// Parses the output of `git worktree list --porcelain -z`: the fields end with a NUL,
/// and an empty field ends a record. The `locked` and `prunable` fields come after the
/// `branch` field, thus the function reads each whole record.
fn parse_worktree_records(text: &str) -> Vec<WorktreeRecord> {
    let mut records = Vec::new();
    let mut current: Option<WorktreeRecord> = None;
    for field in text.split('\0') {
        if field.is_empty() {
            records.extend(current.take());
        } else if let Some(path) = field.strip_prefix("worktree ") {
            records.extend(current.take());
            current = Some(WorktreeRecord {
                path: PathBuf::from(path),
                branch: None,
                locked: false,
                prunable: false,
            });
        } else if let Some(record) = current.as_mut() {
            if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                record.branch = Some(branch.to_string());
            } else if field == "locked" || field.starts_with("locked ") {
                record.locked = true;
            } else if field == "prunable" || field.starts_with("prunable ") {
                record.prunable = true;
            }
        }
    }
    records.extend(current);
    records
}

/// Finds the record of the worktree of the main clone that has `branch` checked out.
fn worktree_on_branch(
    host: &Host,
    main_clone: &Path,
    branch: &str,
) -> anyhow::Result<Option<WorktreeRecord>> {
    let listed = host
        .git()
        .run(main_clone, ["worktree", "list", "--porcelain", "-z"]);
    if !listed.ok {
        bail!(
            "git cannot list the worktrees of {}: {}",
            main_clone.display(),
            listed.stderr
        );
    }
    Ok(parse_worktree_records(&listed.stdout)
        .into_iter()
        .find(|record| record.branch.as_deref() == Some(branch)))
}

/// Refuses a record whose directory a person removed by hand. sidecar never deletes
/// and never prunes, thus the message names the commands for the person.
fn require_live_worktree(
    record: &WorktreeRecord,
    branch: &str,
    main_clone: &Path,
) -> anyhow::Result<()> {
    if !record.prunable && std::fs::symlink_metadata(&record.path).is_ok() {
        return Ok(());
    }
    let path = record.path.display();
    let clone = main_clone.display();
    if record.locked {
        bail!(
            "git lists {path} as the worktree of the branch {branch}, but the directory is missing, and the worktree is locked. \
             A prune does not remove a locked worktree. If the directory is on a disk that is not mounted, mount the disk, and the worktree comes back. \
             Otherwise run `git -C {clone} worktree unlock {path}` and `git -C {clone} worktree prune`, and run the command again. sidecar does not delete or prune."
        );
    }
    bail!(
        "git lists {path} as the worktree of the branch {branch}, but the directory is missing. \
         Run `git -C {clone} worktree prune`, and run the command again. sidecar does not delete or prune."
    )
}

/// True when the reference `name` exists in the repository at `place`.
fn ref_exists(host: &Host, place: &Path, name: &str) -> bool {
    host.git()
        .run(place, ["rev-parse", "--verify", "--quiet", name])
        .ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(path: &str, branch: Option<&str>, locked: bool, prunable: bool) -> WorktreeRecord {
        WorktreeRecord {
            path: PathBuf::from(path),
            branch: branch.map(str::to_string),
            locked,
            prunable,
        }
    }

    #[test]
    fn a_prunable_field_after_the_branch_is_read() {
        let text = "worktree /w/a\0HEAD abc\0branch refs/heads/feature\0prunable gitdir file points to non-existent location\0\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![record("/w/a", Some("feature"), false, true)]
        );
    }

    #[test]
    fn a_locked_field_without_a_reason_is_read() {
        let text = "worktree /w/a\0HEAD abc\0branch refs/heads/feature\0locked\0\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![record("/w/a", Some("feature"), true, false)]
        );
    }

    #[test]
    fn a_locked_field_with_a_reason_is_read() {
        let text = "worktree /w/a\0HEAD abc\0branch refs/heads/feature\0locked on a usb disk\0prunable gone\0\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![record("/w/a", Some("feature"), true, true)]
        );
    }

    #[test]
    fn a_detached_record_has_no_branch() {
        let text = "worktree /w/a\0HEAD abc\0detached\0\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![record("/w/a", None, false, false)]
        );
    }

    #[test]
    fn two_records_stay_apart() {
        let text = "worktree /w/a\0HEAD abc\0branch refs/heads/main\0\0worktree /w/b\0HEAD def\0branch refs/heads/x\0prunable gone\0\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![
                record("/w/a", Some("main"), false, false),
                record("/w/b", Some("x"), false, true),
            ]
        );
    }

    #[test]
    fn a_record_without_a_final_empty_field_is_kept() {
        let text = "worktree /w/a\0branch refs/heads/main\0";
        assert_eq!(
            parse_worktree_records(text),
            vec![record("/w/a", Some("main"), false, false)]
        );
    }
}
