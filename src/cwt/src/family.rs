//! A family of repositories.
//!
//! Some repositories are containers: they track a map of a workspace, and the
//! real repositories sit one level below them, kept out of the parent's history
//! by `.gitignore`. `cwt` treats the parent and its children as one family, so
//! a single listing shows every worktree the user can reach and a single name
//! can select any of them.
//!
//! The family is anchored at a directory: the parent repository if there is
//! one, otherwise the repository the user stands in. Every repository directly
//! below the anchor joins the family. The search stops there — a child of a
//! child is that child's business.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use colored::Colorize;

use crate::worktree::{list_worktrees, paths_equal, RepoWorktrees, Worktree};

/// The indent that puts a worktree under its repository heading.
const GROUP_INDENT: &str = "  ";

/// The branch names that identify the main worktree, in order of priority.
///
/// `main` comes first. `master` is the fallback for a repository that never
/// renamed its first branch.
pub const MAIN_BRANCH_NAMES: [&str; 2] = ["main", "master"];

/// Result of searching for a worktree by name.
#[derive(Debug)]
pub enum WorktreeMatch {
    /// Found exactly one matching worktree.
    Single(usize),
    /// Found multiple matching worktrees (indices into the entry list).
    Multiple(Vec<usize>),
    /// No matching worktree found.
    None,
}

/// How near a repository is to the user, which is the order a name is answered in.
mod rank {
    /// The repository the user is standing in.
    pub const HOME: u8 = 0;
    /// The parent repository the family is anchored at.
    pub const ANCHOR: u8 = 1;
    /// Every other repository in the family.
    pub const OTHER: u8 = 2;
}

/// One repository of the family.
#[derive(Debug, Clone)]
struct Group {
    /// The directory name of the repository's main worktree.
    name: String,
    /// How near this repository is to the user. See [`rank`].
    rank: u8,
}

/// One worktree, and the repository it belongs to.
#[derive(Debug, Clone)]
struct Entry {
    /// Index into the family's groups.
    group: usize,
    /// The worktree itself.
    worktree: Worktree,
}

/// Every worktree of every repository in the family, in display order.
pub struct Family {
    /// The parent repository's worktrees first, then each child repository's.
    entries: Vec<Entry>,
    /// The repositories that contributed, in the same order.
    groups: Vec<Group>,
    /// True when more than one repository contributed, which turns on the
    /// grouped display and the `repo:target` names in messages.
    grouped: bool,
    /// The entry the user is standing in.
    current: Option<usize>,
    /// Repositories that could not be read, for the caller to report.
    warnings: Vec<String>,
}

impl Family {
    /// Discovers every worktree reachable from the repository at `repo_root`.
    ///
    /// With `scan_children` off, the family is just that repository — the
    /// behavior of `cwt` before families existed.
    pub fn discover(repo_root: &Path, scan_children: bool) -> Result<Self, String> {
        let own = list_worktrees(repo_root)?;

        let mut roll = Roll::default();
        let mut warnings = Vec::new();

        if scan_children {
            let anchor_dir = anchor_of(&own.main);
            // The anchor repository leads, then its children by directory name.
            // Standing in a child means the anchor is a different repository, so
            // it has to be read separately. Standing in the anchor itself, the
            // list already in hand is the same one.
            let anchor = if paths_equal(&anchor_dir, &own.main) {
                Ok(own.clone())
            } else {
                list_worktrees(&anchor_dir)
            };
            match anchor {
                Ok(repo) => roll.claim(&repo),
                Err(e) => warnings.push(format!("{}: {e}", anchor_dir.display())),
            }

            for child in child_repo_dirs(&anchor_dir) {
                match list_worktrees(&child) {
                    Ok(repo) => roll.claim(&repo),
                    Err(e) => warnings.push(format!("{}: {e}", child.display())),
                }
            }
        }

        // Without children, and as a backstop if the anchor could not be read,
        // the user's own repository is the family.
        if roll.entries.is_empty() {
            roll.claim(&own);
        }

        let Roll {
            entries,
            mut groups,
            ..
        } = roll;
        let current = find_current(&entries, repo_root);

        // Nearest first: the repository the user stands in, then the anchor —
        // which is the first repository claimed — then everything else.
        if let Some(anchor) = groups.first_mut() {
            anchor.rank = rank::ANCHOR;
        }
        if let Some(index) = current {
            groups[entries[index].group].rank = rank::HOME;
        }

        let grouped = groups.len() > 1;

        Ok(Self {
            entries,
            groups,
            grouped,
            current,
            warnings,
        })
    }

    /// True when the family has no worktrees at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Repositories that could not be read.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The path of the worktree at `index`.
    pub fn path(&self, index: usize) -> &Path {
        &self.entries[index].worktree.path
    }

    /// The worktree after the current one, wrapping around the whole family.
    pub fn next(&self) -> Option<usize> {
        let current = self.current?;
        Some((current + 1) % self.entries.len())
    }

    /// The worktree before the current one, wrapping around the whole family.
    pub fn previous(&self) -> Option<usize> {
        let current = self.current?;
        Some(if current == 0 {
            self.entries.len() - 1
        } else {
            current - 1
        })
    }

    /// The main worktree of the repository the user stands in: the one on
    /// branch `main`, or the one on branch `master` when no worktree of that
    /// repository is on `main`.
    ///
    /// A family holds one main worktree for each repository in it, so the
    /// shortcut has to choose between them. It chooses the repository that owns
    /// the current worktree, which keeps the user inside the repository they
    /// work in instead of sending them to the anchor of the family.
    ///
    /// The branch name must match exactly. The substring match that [`find`]
    /// does is wrong here: in a repository that has no `main` branch, a branch
    /// such as `wt-main-master` would capture the shortcut and send the user
    /// somewhere that is not the main worktree.
    ///
    /// A detached worktree has no branch, so it is never the main worktree.
    ///
    /// [`find`]: Family::find
    pub fn main_worktree(&self) -> Option<usize> {
        let group = self.entries.get(self.current?)?.group;

        MAIN_BRANCH_NAMES.iter().find_map(|name| {
            self.entries
                .iter()
                .position(|e| e.group == group && e.worktree.branch.as_deref() == Some(*name))
        })
    }

    /// How to name the worktree at `index` in a message, so that the name can
    /// be handed straight back to `cwt`.
    pub fn label(&self, index: usize) -> String {
        let entry = &self.entries[index];
        let dir = entry.worktree.dir_name().unwrap_or("<unknown>");
        let branch = entry.worktree.display_branch();
        if self.grouped {
            format!("{}:{dir} [{branch}]", self.groups[entry.group].name)
        } else {
            format!("{dir} [{branch}]")
        }
    }

    /// Every worktree, named the way `label` names them.
    pub fn labels(&self) -> Vec<String> {
        (0..self.entries.len()).map(|i| self.label(i)).collect()
    }

    /// Finds a worktree by name (directory name, branch name, or branch substring).
    ///
    /// The family is searched a repository at a time, nearest first: the
    /// repository the user stands in, then the parent, then the rest. Within a
    /// repository the priority is:
    /// 1. Exact directory name match
    /// 2. Exact branch name match (supports branch names with `/` like `feature/login`)
    ///
    /// An exact match anywhere in the family beats a substring anywhere, so the
    /// substrings are only tried after every repository has been asked for an
    /// exact name. Substrings are then tried nearest first in the same way.
    ///
    /// Rejects names containing `..` or `\` to prevent path traversal. Forward
    /// slashes are allowed since they are common in branch names (for example
    /// `feature/login`) and directory names cannot contain `/` on Unix
    /// filesystems.
    pub fn find(&self, name: &str) -> WorktreeMatch {
        // Reject empty search terms (which would match every branch by
        // substring) and path traversal attempts.
        // Note: `/` is intentionally allowed because:
        // - Branch names commonly contain `/` (for example `feature/login`)
        // - Directory names cannot contain `/` on Unix, so no security risk
        // - Worktree paths come from `git worktree list`, not from user input
        if name.is_empty() || name.contains('\\') || name.contains("..") {
            return WorktreeMatch::None;
        }

        let pool: Vec<usize> = (0..self.entries.len()).collect();
        self.search(&pool, name)
    }

    /// Searches `pool` for `name`, nearest repository first.
    fn search(&self, pool: &[usize], name: &str) -> WorktreeMatch {
        for tier in rank::HOME..=rank::OTHER {
            let near = self.tier(pool, tier);
            if let Some(index) = near
                .iter()
                .copied()
                .find(|&i| self.entries[i].worktree.dir_name() == Some(name))
            {
                return WorktreeMatch::Single(index);
            }
            if let Some(index) = near
                .iter()
                .copied()
                .find(|&i| self.entries[i].worktree.branch.as_deref() == Some(name))
            {
                return WorktreeMatch::Single(index);
            }
        }

        // No exact name anywhere in the family. Every substring match in the
        // nearest repository that has one is collected, because they all have to
        // be shown when there is more than one.
        let wanted = name.to_lowercase();
        for tier in rank::HOME..=rank::OTHER {
            let matches: Vec<usize> = self
                .tier(pool, tier)
                .into_iter()
                .filter(|&i| {
                    self.entries[i]
                        .worktree
                        .branch
                        .as_ref()
                        .is_some_and(|branch| branch.to_lowercase().contains(&wanted))
                })
                .collect();

            match matches.len() {
                0 => {}
                1 => return WorktreeMatch::Single(matches[0]),
                _ => return WorktreeMatch::Multiple(matches),
            }
        }

        WorktreeMatch::None
    }

    /// The entries of `pool` that belong to a repository of the given rank.
    fn tier(&self, pool: &[usize], rank: u8) -> Vec<usize> {
        pool.iter()
            .copied()
            .filter(|&index| self.groups[self.entries[index].group].rank == rank)
            .collect()
    }

    /// Renders the listing, with the current worktree highlighted.
    ///
    /// One repository prints as a plain list. More than one prints grouped, with
    /// each repository's name above its worktrees.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut heading: Option<&str> = None;

        for (index, entry) in self.entries.iter().enumerate() {
            let repo = self.groups[entry.group].name.as_str();
            if self.grouped && heading != Some(repo) {
                if heading.is_some() {
                    out.push('\n');
                }
                let _ = writeln!(out, "{}", repo.bold());
                heading = Some(repo);
            }

            let is_current = self.current == Some(index);
            let marker = if is_current { ">" } else { " " };
            let indent = if self.grouped { GROUP_INDENT } else { "" };
            let path = entry.worktree.path.display().to_string();
            let branch = entry.worktree.display_branch();

            let _ = if is_current {
                writeln!(
                    out,
                    "{} {indent}{} [{}]",
                    marker.green().bold(),
                    path.green().bold(),
                    branch.green()
                )
            } else {
                writeln!(out, "{marker} {indent}{path} [{}]", branch.dimmed())
            };
        }

        out
    }
}

/// The directory whose children make up the family.
///
/// A repository checked out inside another repository is a child of that
/// parent, so the family is anchored one level up. Anywhere else, the
/// repository anchors its own family.
fn anchor_of(main_worktree: &Path) -> PathBuf {
    let parent = main_worktree.parent();
    match parent {
        Some(parent) if parent.join(".git").exists() => parent.to_path_buf(),
        _ => main_worktree.to_path_buf(),
    }
}

/// The directories one level below `dir` that are git repositories or worktrees,
/// sorted by name.
fn child_repo_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut children: Vec<PathBuf> = read
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join(".git").exists())
        .collect();
    children.sort();
    children
}

/// The family under construction.
#[derive(Default)]
struct Roll {
    /// The worktrees claimed so far, in display order.
    entries: Vec<Entry>,
    /// The repositories that claimed them, in the same order.
    groups: Vec<Group>,
    /// The main worktree of every repository already claimed.
    claimed: Vec<PathBuf>,
}

impl Roll {
    /// Adds a repository's worktrees to the family, unless another repository
    /// already claimed them.
    ///
    /// A repository is claimed by its main worktree, so a directory that is
    /// really a linked worktree of a repository already in the family adds
    /// nothing new.
    fn claim(&mut self, repo: &RepoWorktrees) {
        let key = canonical(&repo.main);
        if self.claimed.iter().any(|seen| paths_equal(seen, &key)) {
            return;
        }
        self.claimed.push(key);

        let group = self.groups.len();
        self.groups.push(Group {
            name: repo
                .main
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<unknown>")
                .to_string(),
            rank: rank::OTHER,
        });

        for worktree in &repo.all {
            self.entries.push(Entry {
                group,
                worktree: worktree.clone(),
            });
        }
    }
}

/// Finds the entry for the worktree the user is standing in.
fn find_current(entries: &[Entry], repo_root: &Path) -> Option<usize> {
    let here = std::fs::canonicalize(repo_root).ok()?;
    entries.iter().position(|entry| {
        std::fs::canonicalize(&entry.worktree.path).is_ok_and(|path| paths_equal(&path, &here))
    })
}

/// The resolved form of a path, falling back to the path itself when it cannot
/// be resolved (a worktree whose directory was deleted, for example).
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a family by hand, without touching the filesystem.
    ///
    /// Each entry is `(repository, path, branch)`. Repositories are ranked the
    /// way `discover` ranks them: the first repository named is the anchor, and
    /// the one holding the current worktree is home.
    fn family(entries: Vec<(&str, &str, &str)>, grouped: bool, current: Option<usize>) -> Family {
        let entries = entries
            .into_iter()
            .map(|(repo, path, branch)| (repo, path, Some(branch)))
            .collect();
        detachable_family(entries, grouped, current)
    }

    /// The same, for a family that has to hold a detached worktree.
    ///
    /// Pass `None` as the branch for a detached HEAD.
    fn detachable_family(
        entries: Vec<(&str, &str, Option<&str>)>,
        grouped: bool,
        current: Option<usize>,
    ) -> Family {
        let mut groups: Vec<Group> = Vec::new();
        let entries: Vec<Entry> = entries
            .into_iter()
            .map(|(repo, path, branch)| {
                let group = groups
                    .iter()
                    .position(|g| g.name == repo)
                    .unwrap_or_else(|| {
                        groups.push(Group {
                            name: repo.to_string(),
                            rank: rank::OTHER,
                        });
                        groups.len() - 1
                    });
                Entry {
                    group,
                    worktree: Worktree {
                        path: PathBuf::from(path),
                        head: "abc1234567890".to_string(),
                        branch: branch.map(str::to_string),
                    },
                }
            })
            .collect();

        if let Some(anchor) = groups.first_mut() {
            anchor.rank = rank::ANCHOR;
        }
        if let Some(index) = current {
            groups[entries[index].group].rank = rank::HOME;
        }

        Family {
            entries,
            groups,
            grouped,
            current,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn main_worktree_picks_main() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/wt1", "feature"),
            ],
            false,
            Some(1),
        );
        assert_eq!(one.main_worktree(), Some(0));
    }

    #[test]
    fn main_worktree_falls_back_to_master() {
        // A repository that never renamed master still has a main worktree.
        let one = family(
            vec![
                ("solo", "/repo-wt/wt1", "feature"),
                ("solo", "/repo", "master"),
            ],
            false,
            Some(0),
        );
        assert_eq!(one.main_worktree(), Some(1));
    }

    #[test]
    fn main_worktree_prefers_main_over_master() {
        // Both branches exist. main wins, whatever the order of the list.
        let one = family(
            vec![
                ("solo", "/repo-wt/old", "master"),
                ("solo", "/repo", "main"),
            ],
            false,
            Some(0),
        );
        assert_eq!(one.main_worktree(), Some(1));
    }

    #[test]
    fn main_worktree_ignores_substring_branches() {
        // The branch name must match exactly. A branch that merely contains
        // "main" must not capture the main worktree of a master repository.
        let one = family(
            vec![
                ("solo", "/repo-wt/wt1", "wt-main-master"),
                ("solo", "/repo", "master"),
            ],
            false,
            Some(0),
        );
        assert_eq!(one.main_worktree(), Some(1));
    }

    #[test]
    fn main_worktree_ignores_detached_head() {
        let one = detachable_family(
            vec![
                ("solo", "/repo-wt/wt1", None),
                ("solo", "/repo", Some("master")),
            ],
            false,
            Some(0),
        );
        assert_eq!(one.main_worktree(), Some(1));
    }

    #[test]
    fn main_worktree_reports_nothing_without_main_or_master() {
        let one = family(
            vec![
                ("solo", "/repo", "trunk"),
                ("solo", "/repo-wt/wt1", "wt-main-master"),
            ],
            false,
            Some(0),
        );
        assert_eq!(one.main_worktree(), None);
    }

    #[test]
    fn main_worktree_stays_in_the_repository_the_user_stands_in() {
        // Every repository of a family has a main worktree of its own. The
        // shortcut must not send the user out of the repository they work in.
        let whole = family(
            vec![
                ("parent", "/p", "main"),
                ("child-a", "/p/child-a", "main"),
                ("child-b", "/p/child-b", "master"),
                ("child-b", "/p/child-b-wt/feature", "feature"),
            ],
            true,
            Some(3),
        );
        assert_eq!(whole.main_worktree(), Some(2));
    }

    #[test]
    fn main_worktree_reports_nothing_when_the_current_worktree_is_unknown() {
        // Without a current worktree there is no repository to stay inside of.
        let whole = family(
            vec![("parent", "/p", "main"), ("child", "/p/child", "main")],
            true,
            None,
        );
        assert_eq!(whole.main_worktree(), None);
    }

    #[test]
    fn anchor_stays_put_when_the_parent_is_not_a_repository() {
        // /tmp is not a repository, so a repository directly inside it anchors
        // its own family.
        let root = std::env::temp_dir();
        let repo = root.join("no-parent-repo-should-exist-here");
        assert_eq!(anchor_of(&repo), repo);
    }

    #[test]
    fn render_of_one_repository_has_no_headings() {
        let one = family(
            vec![("solo", "/repo", "main"), ("solo", "/repo-wt/x", "feature")],
            false,
            Some(0),
        );
        let rendered = one.render();
        assert_eq!(
            rendered, "> /repo [main]\n  /repo-wt/x [feature]\n",
            "a lone repository prints exactly as it always has"
        );
    }

    #[test]
    fn render_of_a_family_heads_each_group_with_its_repository() {
        let two = family(
            vec![("parent", "/p", "main"), ("child", "/p/c", "trunk")],
            true,
            Some(1),
        );
        assert_eq!(
            two.render(),
            "parent\n    /p [main]\n\nchild\n>   /p/c [trunk]\n",
            "each repository heads its own group, and the marker keeps its column"
        );
    }

    #[test]
    fn label_names_the_repository_only_when_there_is_more_than_one() {
        let one = family(vec![("solo", "/repo-wt/x", "feature")], false, None);
        assert_eq!(one.label(0), "x [feature]");

        let two = family(vec![("child", "/p/c-wt/x", "feature")], true, None);
        assert_eq!(two.label(0), "child:x [feature]");
    }

    #[test]
    fn find_prefers_a_directory_name_to_a_branch_name() {
        let two = family(
            vec![("a", "/a/thing", "main"), ("b", "/b/other", "thing")],
            true,
            None,
        );
        assert!(matches!(two.find("thing"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_rejects_path_traversal_and_the_empty_name() {
        let one = family(vec![("solo", "/repo", "main")], false, None);
        assert!(matches!(one.find(""), WorktreeMatch::None));
        assert!(matches!(one.find(".."), WorktreeMatch::None));
        assert!(matches!(one.find("../etc/passwd"), WorktreeMatch::None));
        assert!(matches!(one.find("foo\\bar"), WorktreeMatch::None));
    }

    #[test]
    fn find_matches_an_exact_directory_name() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/absurd-rock", "feature"),
            ],
            false,
            None,
        );
        assert!(matches!(one.find("absurd-rock"), WorktreeMatch::Single(1)));
    }

    #[test]
    fn find_matches_an_exact_branch_name() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/absurd-rock", "feature"),
            ],
            false,
            None,
        );
        assert!(matches!(one.find("feature"), WorktreeMatch::Single(1)));
        assert!(matches!(one.find("main"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_reports_nothing_for_an_unknown_name() {
        let one = family(vec![("solo", "/repo", "main")], false, None);
        assert!(matches!(one.find("nonexistent"), WorktreeMatch::None));
    }

    #[test]
    fn find_allows_forward_slashes_in_branch_names() {
        // Forward slashes are common in branch names (feature/*, bugfix/*) and
        // must work for both exact and substring matching.
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/wt1", "feature/user-auth"),
                ("solo", "/repo-wt/wt2", "feature/login-page"),
            ],
            false,
            None,
        );

        assert!(matches!(
            one.find("feature/user-auth"),
            WorktreeMatch::Single(1)
        ));

        match one.find("feature/") {
            WorktreeMatch::Multiple(indices) => {
                assert_eq!(indices.len(), 2);
                assert!(indices.contains(&1));
                assert!(indices.contains(&2));
            }
            other => panic!("Expected Multiple, got {other:?}"),
        }

        assert!(matches!(one.find("ure/user"), WorktreeMatch::Single(1)));
    }

    #[test]
    fn find_matches_one_branch_by_substring() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/wt1", "feature/login-page"),
                ("solo", "/repo-wt/wt2", "bugfix/header"),
            ],
            false,
            None,
        );
        assert!(matches!(one.find("login"), WorktreeMatch::Single(1)));
        assert!(matches!(one.find("LOGIN"), WorktreeMatch::Single(1)));
        assert!(matches!(one.find("header"), WorktreeMatch::Single(2)));
    }

    #[test]
    fn find_reports_every_branch_a_substring_matches() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/wt1", "feature/login-page"),
                ("solo", "/repo-wt/wt2", "feature/logout-page"),
            ],
            false,
            None,
        );
        match one.find("feature") {
            WorktreeMatch::Multiple(indices) => assert_eq!(indices.len(), 2),
            other => panic!("Expected Multiple, got {other:?}"),
        }
        match one.find("page") {
            WorktreeMatch::Multiple(indices) => assert_eq!(indices.len(), 2),
            other => panic!("Expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn find_prefers_an_exact_branch_to_a_substring() {
        let one = family(
            vec![
                ("solo", "/repo", "main"),
                ("solo", "/repo-wt/wt1", "main-feature"),
            ],
            false,
            None,
        );
        // "main" is exactly one branch and part of another. The exact one wins.
        assert!(matches!(one.find("main"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_ignores_case_in_a_substring() {
        let one = family(vec![("solo", "/repo", "Feature/UserAuth")], false, None);
        assert!(matches!(one.find("userauth"), WorktreeMatch::Single(0)));
        assert!(matches!(one.find("USERAUTH"), WorktreeMatch::Single(0)));
        assert!(matches!(one.find("UserAuth"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_prefers_the_home_repository_to_the_parent() {
        // Both repositories have a `main` branch. The user stands in the child.
        let two = family(
            vec![("parent", "/p", "main"), ("child", "/p/c", "main")],
            true,
            Some(1),
        );
        assert!(matches!(two.find("main"), WorktreeMatch::Single(1)));

        // Standing in the parent, the same name answers from the parent.
        let two = family(
            vec![("parent", "/p", "main"), ("child", "/p/c", "main")],
            true,
            Some(0),
        );
        assert!(matches!(two.find("main"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_prefers_the_parent_to_an_unrelated_repository() {
        // Neither the parent nor the sibling is home, and both have `shared`.
        let three = family(
            vec![
                ("parent", "/p", "shared"),
                ("home", "/p/home", "work"),
                ("other", "/p/other", "shared"),
            ],
            true,
            Some(1),
        );
        assert!(matches!(three.find("shared"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn find_prefers_an_exact_name_elsewhere_to_a_substring_at_home() {
        // Home has `beta-old`, which contains "beta". Another repository has the
        // branch `beta` itself. The exact name wins even though it is further away.
        let two = family(
            vec![("home", "/h", "beta-old"), ("other", "/h/o", "beta")],
            true,
            Some(0),
        );
        assert!(matches!(two.find("beta"), WorktreeMatch::Single(1)));
    }

    #[test]
    fn find_prefers_a_substring_at_home_to_a_substring_elsewhere() {
        let two = family(
            vec![("home", "/h", "feature-x"), ("other", "/h/o", "feature-y")],
            true,
            Some(0),
        );
        assert!(matches!(two.find("feature"), WorktreeMatch::Single(0)));
    }

    #[test]
    fn cycling_reports_nothing_when_the_current_worktree_is_unknown() {
        let one = family(vec![("solo", "/repo", "main")], false, None);
        assert_eq!(one.next(), None);
        assert_eq!(one.previous(), None);
    }

    #[test]
    fn cycling_wraps_around_the_whole_family() {
        let three = family(
            vec![
                ("a", "/a", "main"),
                ("b", "/a/b", "main"),
                ("c", "/a/c", "main"),
            ],
            true,
            Some(2),
        );
        assert_eq!(three.next(), Some(0), "the last entry wraps to the first");
        assert_eq!(three.previous(), Some(1));
    }
}
