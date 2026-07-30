use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use repowalker::find_git_repo;

/// Exit codes for different error conditions.
mod exit_codes {
    /// Not in a git repository.
    pub const NOT_IN_REPO: i32 = 1;
    /// A git command failed to execute or returned an error.
    pub const GIT_COMMAND_ERROR: i32 = 2;
    /// No worktree matched the target.
    pub const WORKTREE_NOT_FOUND: i32 = 3;
    /// The target matched more than one worktree.
    pub const MULTIPLE_MATCHES: i32 = 4;
    /// The worktree contains submodules and `--force` was not given.
    pub const SUBMODULES_PRESENT: i32 = 5;
    /// The shell is standing inside the worktree it was asked to nuke.
    pub const INSIDE_TARGET: i32 = 6;
    /// The worktree was removed but its branch could not be deleted.
    pub const BRANCH_NOT_DELETED: i32 = 7;
}

/// The index mode git records for a submodule (gitlink) entry.
const GITLINK_MODE_PREFIX: &str = "160000 ";

/// Remove a git worktree and force-delete its branch.
///
/// The target names a worktree, not a branch to delete in isolation: gitnuke
/// resolves it against `git worktree list`, so the branch it deletes is
/// whatever that worktree had checked out. A detached-HEAD worktree is removed
/// with no branch deletion.
///
/// # Usage
///
/// ```sh
/// gitnuke ../feature-wt        # by path
/// gitnuke feature-wt           # by directory name
/// gitnuke issue-42             # by branch name
/// ```
///
/// # Exit Codes
///
/// - 0: Success
/// - 1: Not in a git repository
/// - 2: A git command failed
/// - 3: No worktree matched the target
/// - 4: The target matched more than one worktree
/// - 5: The worktree contains submodules and `--force` was not given
/// - 6: The shell is standing inside the target worktree
/// - 7: The worktree was removed but its branch could not be deleted
#[derive(Parser)]
#[command(name = "gitnuke")]
#[command(about = "Remove a git worktree and force-delete its branch")]
#[command(version = version_string!())]
struct Cli {
    /// Worktree to nuke: its path, its directory name, or its branch name.
    #[arg(required = true)]
    targets: Vec<String>,

    /// Nuke the worktree even if git would refuse to remove it.
    ///
    /// Required for a worktree containing submodules, which git otherwise
    /// refuses outright, and for one holding uncommitted changes. Both cases
    /// discard work that exists nowhere else — including anything uncommitted
    /// or unpushed inside the submodule checkouts.
    #[arg(short = 'f', long, verbatim_doc_comment)]
    force: bool,

    /// Keep the branch unless it is fully merged (`git branch -d`).
    ///
    /// Only affects the branch: the worktree is still removed. Without this,
    /// gitnuke force-deletes the branch (`git branch -D`) whether or not its
    /// commits landed anywhere.
    #[arg(short = 's', long, verbatim_doc_comment)]
    safe: bool,

    /// Report what would happen without removing or deleting anything.
    ///
    /// Runs the same checks as a real run, so a target that would be refused is
    /// still reported as a failure.
    #[arg(short = 'n', long, verbatim_doc_comment)]
    dry_run: bool,
}

/// How a nuke should behave, independent of which worktree it is aimed at.
#[derive(Debug, Clone, Copy)]
struct NukeOptions {
    /// Override git's refusal to remove worktrees with submodules or changes.
    force: bool,
    /// Delete the branch only if it is fully merged.
    safe: bool,
    /// Check everything, change nothing.
    dry_run: bool,
}

impl From<&Cli> for NukeOptions {
    fn from(cli: &Cli) -> Self {
        NukeOptions {
            force: cli.force,
            safe: cli.safe,
            dry_run: cli.dry_run,
        }
    }
}

/// The name of a git branch, always with any `refs/heads/` prefix stripped.
///
/// gitnuke's most destructive call — `git branch -D` — takes one of these, and
/// it sits right next to a worktree path in that signature. Keeping the two as
/// distinct types makes swapping them a compile error rather than a deleted
/// branch. Construction goes through [`BranchName::from_ref`], so no caller can
/// forget to strip the prefix and end up asking git to delete `refs/heads/x`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BranchName(String);

impl BranchName {
    /// Builds a branch name from a git ref, stripping a leading `refs/heads/`.
    ///
    /// A ref that carries no such prefix (already a short name) is taken as-is.
    fn from_ref(reference: &str) -> Self {
        BranchName(
            reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string(),
        )
    }

    /// The short branch name, as git's own `branch` subcommand wants it.
    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The filesystem path of a git worktree.
///
/// Distinct from a plain `PathBuf` so it cannot be confused with the repository
/// root (which every git invocation here also takes) or with a [`BranchName`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorktreePath(PathBuf);

impl WorktreePath {
    /// Wraps a path reported by git as a worktree location.
    fn new(path: impl Into<PathBuf>) -> Self {
        WorktreePath(path.into())
    }

    /// The path itself, for handing to `git` or the filesystem.
    fn as_path(&self) -> &Path {
        &self.0
    }

    /// The final path component (e.g. `absurd-rock` from a full path).
    fn dir_name(&self) -> Option<&str> {
        self.0.file_name()?.to_str()
    }
}

impl fmt::Display for WorktreePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}

/// Represents a single git worktree.
#[derive(Debug, Clone)]
struct Worktree {
    /// The filesystem path to this worktree.
    path: WorktreePath,
    /// The branch checked out here, or None for detached HEAD.
    branch: Option<BranchName>,
}

/// Parses the output of `git worktree list --porcelain`.
///
/// The porcelain format looks like:
/// ```text
/// worktree /path/to/repo
/// HEAD abc123...
/// branch refs/heads/main
///
/// worktree /path/to/worktree
/// HEAD def456...
/// detached
/// ```
///
/// For a detached HEAD the `branch` line is absent. The main worktree is always
/// the first block, and the order here is preserved so callers can rely on it.
fn parse_worktree_list(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<WorktreePath> = None;
    let mut current_branch: Option<BranchName> = None;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // A new block starts: flush the previous one.
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(WorktreePath::new(path));
        } else if let Some(branch) = line.strip_prefix("branch ") {
            // BranchName::from_ref owns the `refs/heads/` stripping.
            current_branch = Some(BranchName::from_ref(branch));
        }
        // Ignore HEAD/bare/detached/locked/prunable lines.
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
        });
    }

    worktrees
}

/// Lists the worktrees of the repository containing `repo_root`.
fn get_worktrees(repo_root: &Path) -> Result<Vec<Worktree>, String> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("failed to execute git: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "git worktree list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_worktree_list(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Result of resolving a target string against the worktree list.
#[derive(Debug, PartialEq, Eq)]
enum Resolution {
    /// Exactly one worktree matched (index into the list).
    Single(usize),
    /// Several worktrees matched (indices into the list).
    Multiple(Vec<usize>),
    /// Nothing matched.
    NotFound,
}

/// Resolves a target to exactly one worktree.
///
/// Matching is deliberately **exact only** — path, then directory name, then
/// branch name. gitnuke destroys whatever it resolves, so it must never guess:
/// a substring match like cwt's would happily nuke `issue-421` when asked for
/// `issue-42`.
///
/// `cwd` is the directory relative paths are resolved against.
fn resolve_target(worktrees: &[Worktree], target: &str, cwd: Option<&Path>) -> Resolution {
    if target.is_empty() {
        return Resolution::NotFound;
    }

    // First: a path that points at one of the worktrees. Canonicalizing both
    // sides makes `.`, `..`, trailing slashes, and symlinked temp dirs all
    // resolve to the same thing.
    if let Some(canonical) = canonicalize_target(target, cwd) {
        if let Some(idx) = worktrees.iter().position(|wt| {
            wt.path
                .as_path()
                .canonicalize()
                .is_ok_and(|p| paths_equal(&p, &canonical))
        }) {
            return Resolution::Single(idx);
        }
    }

    // Then: an exact directory name or branch name. Both passes are folded
    // together so a name that hits one worktree's directory and another's
    // branch is reported as ambiguous instead of silently preferring one.
    let hits: Vec<usize> = worktrees
        .iter()
        .enumerate()
        .filter(|(_, wt)| {
            wt.path.dir_name() == Some(target)
                || wt.branch.as_ref().map(BranchName::as_str) == Some(target)
        })
        .map(|(idx, _)| idx)
        .collect();

    match hits.len() {
        0 => Resolution::NotFound,
        1 => Resolution::Single(hits[0]),
        _ => Resolution::Multiple(hits),
    }
}

/// Canonicalizes a possibly-relative target path, or None if it doesn't exist.
fn canonicalize_target(target: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let as_path = Path::new(target);
    let absolute = if as_path.is_absolute() {
        as_path.to_path_buf()
    } else {
        cwd?.join(as_path)
    };
    absolute.canonicalize().ok()
}

/// Compares two paths, handling case-insensitivity on macOS.
fn paths_equal(a: &Path, b: &Path) -> bool {
    #[cfg(target_os = "macos")]
    {
        // The default macOS filesystem is case-insensitive.
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    }

    #[cfg(not(target_os = "macos"))]
    {
        a == b
    }
}

/// What is standing between a worktree and its removal, submodule-wise.
///
/// This mirrors git's own `validate_no_submodules` check, which is what produces
/// "working trees containing submodules cannot be moved or removed": a worktree
/// blocks removal if its index holds a gitlink whose directory exists on disk,
/// or if its private git dir has a `modules/` directory. Reproducing the check
/// rather than parsing git's refusal keeps the diagnosis locale-independent and
/// lets gitnuke name the submodules that are in the way.
#[derive(Debug, Default, PartialEq, Eq)]
struct SubmoduleReport {
    /// Submodule paths, relative to the worktree, that are checked out.
    paths: Vec<String>,
    /// Whether the worktree's private git dir contains submodule metadata.
    has_module_metadata: bool,
}

impl SubmoduleReport {
    /// Whether git will refuse a plain `git worktree remove` because of this.
    fn blocks_removal(&self) -> bool {
        !self.paths.is_empty() || self.has_module_metadata
    }

    /// Human-readable description of what is in the way.
    fn describe(&self) -> String {
        if self.paths.is_empty() {
            "submodule metadata".to_string()
        } else {
            format!("submodules ({})", self.paths.join(", "))
        }
    }
}

/// Extracts the submodule (gitlink) paths from `git ls-files --stage -z` output.
///
/// Each NUL-separated entry looks like `<mode> <object> <stage>\t<path>`. `-z`
/// is what makes this safe for non-ASCII and whitespace-bearing paths: without
/// it git would quote and escape them according to `core.quotePath`.
fn parse_gitlink_paths(stdout: &str) -> Vec<String> {
    stdout
        .split('\0')
        .filter_map(|entry| {
            let (metadata, path) = entry.split_once('\t')?;
            metadata
                .starts_with(GITLINK_MODE_PREFIX)
                .then(|| path.to_string())
        })
        .collect()
}

/// Inspects `worktree` for submodules that would block its removal.
///
/// A worktree git can no longer inspect (already deleted, corrupt) yields an
/// empty report: there is nothing to protect, and `git worktree remove` will
/// give the authoritative answer either way.
fn find_submodules(worktree: &WorktreePath) -> SubmoduleReport {
    let mut paths = Vec::new();
    if let Ok(output) = Command::new("git")
        .args(["ls-files", "--stage", "-z"])
        .current_dir(worktree.as_path())
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // git only refuses when the submodule is actually checked out; a
            // gitlink with no directory on disk is inert.
            paths = parse_gitlink_paths(&stdout)
                .into_iter()
                .filter(|path| worktree.as_path().join(path).exists())
                .collect();
        }
    }

    SubmoduleReport {
        has_module_metadata: worktree_git_dir(worktree)
            .is_some_and(|git_dir| git_dir.join("modules").is_dir()),
        paths,
    }
}

/// The worktree's private git dir (`.../.git/worktrees/<name>` for a linked one).
fn worktree_git_dir(worktree: &WorktreePath) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--absolute-git-dir"])
        .current_dir(worktree.as_path())
        .output()
        .ok()?;

    output.status.success().then(|| {
        PathBuf::from(
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches('\n')
                .to_string(),
        )
    })
}

/// A failure to nuke one target: the message to print and the exit code to use.
struct NukeError {
    code: i32,
    message: String,
}

impl NukeError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        NukeError {
            code,
            message: message.into(),
        }
    }
}

/// Removes a worktree, then deletes the branch it had checked out.
///
/// The branch is only deleted once the removal has actually succeeded, so a
/// refused removal never leaves the branch destroyed and the worktree standing.
fn nuke(repo_root: &Path, worktree: &Worktree, options: NukeOptions) -> Result<(), NukeError> {
    let submodules = find_submodules(&worktree.path);
    if submodules.blocks_removal() && !options.force {
        return Err(NukeError::new(
            exit_codes::SUBMODULES_PRESENT,
            format!(
                "{} contains {} — git refuses to remove a worktree with submodules \
                 checked out.\n  Nuking it deletes those checkouts along with any \
                 uncommitted or unpushed work inside them.\n  Re-run with --force to \
                 nuke it anyway.",
                worktree.path,
                submodules.describe(),
            ),
        ));
    }

    if options.dry_run {
        return report_plan(worktree, &submodules, options);
    }

    let mut args = vec!["worktree", "remove"];
    if options.force {
        // One --force is enough for both of git's refusals: submodules present
        // and uncommitted changes. (Verified against git 2.55.)
        args.push("--force");
    }

    let output = Command::new("git")
        .args(args)
        .arg(worktree.path.as_path())
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            NukeError::new(
                exit_codes::GIT_COMMAND_ERROR,
                format!("failed to execute git: {e}"),
            )
        })?;

    if !output.status.success() {
        return Err(NukeError::new(
            exit_codes::GIT_COMMAND_ERROR,
            format!(
                "could not remove worktree {}: {}",
                worktree.path,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    println!(
        "{} removed worktree {}",
        "gitnuke:".green().bold(),
        worktree.path
    );

    let Some(branch) = &worktree.branch else {
        println!(
            "{} {} had a detached HEAD, so there is no branch to delete",
            "gitnuke:".green().bold(),
            worktree.path
        );
        return Ok(());
    };

    delete_branch(repo_root, branch, options.safe)
}

/// Prints what a real run would do, and returns Ok since nothing was touched.
///
/// Reached only after every gate has passed, so a refused target reports its
/// refusal instead: a dry run is a preflight, not a description.
fn report_plan(
    worktree: &Worktree,
    submodules: &SubmoduleReport,
    options: NukeOptions,
) -> Result<(), NukeError> {
    let extra = if submodules.blocks_removal() {
        format!(" (discarding its {})", submodules.describe())
    } else {
        String::new()
    };
    println!(
        "{} would remove worktree {}{extra}",
        "gitnuke:".yellow().bold(),
        worktree.path
    );

    match &worktree.branch {
        Some(branch) => println!(
            "{} would delete branch {branch} (git branch {})",
            "gitnuke:".yellow().bold(),
            if options.safe { "-d" } else { "-D" }
        ),
        None => println!(
            "{} {} has a detached HEAD, so no branch would be deleted",
            "gitnuke:".yellow().bold(),
            worktree.path
        ),
    }

    Ok(())
}

/// Deletes a branch, echoing git's own report.
///
/// `safe` picks `git branch -d` (refuses an unmerged branch) over `-D`.
fn delete_branch(repo_root: &Path, branch: &BranchName, safe: bool) -> Result<(), NukeError> {
    let delete_flag = if safe { "-d" } else { "-D" };
    let output = Command::new("git")
        .args(["branch", delete_flag, branch.as_str()])
        .current_dir(repo_root)
        .output()
        .map_err(|e| {
            NukeError::new(
                exit_codes::GIT_COMMAND_ERROR,
                format!("failed to execute git: {e}"),
            )
        })?;

    if !output.status.success() {
        return Err(NukeError::new(
            exit_codes::BRANCH_NOT_DELETED,
            format!(
                "worktree removed, but branch '{branch}' was kept: {}\n  \
                 Delete it anyway with: git branch -D {branch}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    println!(
        "{} {}",
        "gitnuke:".green().bold(),
        String::from_utf8_lossy(&output.stdout).trim()
    );
    Ok(())
}

/// Renders the "no worktree matched" message, listing what is available.
fn not_found_message(worktrees: &[Worktree], target: &str) -> String {
    let mut message = format!("no worktree matches '{target}'. Known worktrees:");
    for wt in worktrees {
        let branch = wt
            .branch
            .as_ref()
            .map_or("detached HEAD", BranchName::as_str);
        message.push_str(&format!("\n  {} [{branch}]", wt.path));
    }
    message
}

/// Whether `cwd` is the worktree at `worktree`, or somewhere beneath it.
///
/// Both sides are canonicalized so `..` segments, trailing slashes, and
/// symlinked parents (macOS `/var` → `/private/var`) cannot hide the overlap.
fn cwd_is_inside(cwd: Option<&Path>, worktree: &WorktreePath) -> bool {
    let Some(Ok(cwd)) = cwd.map(Path::canonicalize) else {
        return false;
    };
    let Ok(worktree) = worktree.as_path().canonicalize() else {
        return false;
    };

    paths_equal(&cwd, &worktree) || cwd.starts_with(&worktree)
}

/// Nukes one target, resolving it against a freshly listed set of worktrees.
fn nuke_target(
    repo_root: &Path,
    target: &str,
    cwd: Option<&Path>,
    options: NukeOptions,
) -> Result<(), NukeError> {
    let worktrees =
        get_worktrees(repo_root).map_err(|e| NukeError::new(exit_codes::GIT_COMMAND_ERROR, e))?;

    match resolve_target(&worktrees, target, cwd) {
        Resolution::Single(idx) => {
            let worktree = &worktrees[idx];

            // Removing the directory the shell is sitting in leaves that shell
            // in a deleted cwd, where every later git command fails
            // confusingly. gitnuke is a binary, not a shell function, so it
            // cannot cd the caller out — refusing is the only safe answer, and
            // --force does not change that: it overrides git's refusals, not
            // the caller's shell.
            if cwd_is_inside(cwd, &worktree.path) {
                // worktrees[0] is always the main worktree in git's listing.
                let elsewhere = worktrees
                    .first()
                    .map_or_else(String::new, |main| format!(" (for example {})", main.path));
                return Err(NukeError::new(
                    exit_codes::INSIDE_TARGET,
                    format!(
                        "you are inside {} — cd somewhere else first{elsewhere}, \
                         otherwise your shell is left in a deleted directory",
                        worktree.path
                    ),
                ));
            }

            nuke(repo_root, worktree, options)
        }
        Resolution::Multiple(indices) => {
            let mut message =
                format!("'{target}' matches more than one worktree; use a path instead:");
            for idx in indices {
                message.push_str(&format!("\n  {}", worktrees[idx].path));
            }
            Err(NukeError::new(exit_codes::MULTIPLE_MATCHES, message))
        }
        Resolution::NotFound => Err(NukeError::new(
            exit_codes::WORKTREE_NOT_FOUND,
            not_found_message(&worktrees, target),
        )),
    }
}

fn main() {
    let cli = Cli::parse();

    let Some(repo_root) = find_git_repo() else {
        eprintln!("{} not in a git repository", "gitnuke:".red().bold());
        exit(exit_codes::NOT_IN_REPO);
    };

    let cwd = std::env::current_dir().ok();
    let options = NukeOptions::from(&cli);

    // The worktree list is re-read per target because nuking one invalidates it.
    let mut first_error: Option<i32> = None;
    for target in &cli.targets {
        if let Err(error) = nuke_target(&repo_root, target, cwd.as_deref(), options) {
            eprintln!("{} {}", "gitnuke:".red().bold(), error.message);
            first_error.get_or_insert(error.code);
        }
    }

    if let Some(code) = first_error {
        exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: WorktreePath::new(path),
            branch: branch.map(BranchName::from_ref),
        }
    }

    /// The short branch name a worktree carries, as a plain `&str`.
    fn branch_of(worktree: &Worktree) -> Option<&str> {
        worktree.branch.as_ref().map(BranchName::as_str)
    }

    #[test]
    fn parses_porcelain_worktree_list() {
        let output = "\
worktree /repo
HEAD abc123
branch refs/heads/main

worktree /repo-worktrees/feature
HEAD def456
branch refs/heads/feature/login

worktree /repo-worktrees/detached
HEAD 789abc
detached
";
        let worktrees = parse_worktree_list(output);

        assert_eq!(worktrees.len(), 3);
        assert_eq!(worktrees[0].path.as_path(), Path::new("/repo"));
        assert_eq!(branch_of(&worktrees[0]), Some("main"));
        assert_eq!(branch_of(&worktrees[1]), Some("feature/login"));
        assert_eq!(branch_of(&worktrees[2]), None);
    }

    #[test]
    fn parses_final_block_without_trailing_blank_line() {
        let worktrees = parse_worktree_list("worktree /repo\nHEAD abc\nbranch refs/heads/main");

        assert_eq!(worktrees.len(), 1);
        assert_eq!(branch_of(&worktrees[0]), Some("main"));
    }

    #[test]
    fn resolves_exact_directory_and_branch_names() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/absurd-rock", Some("issue-42")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "absurd-rock", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "issue-42", None),
            Resolution::Single(1)
        );
    }

    #[test]
    fn never_resolves_a_substring_of_a_branch_name() {
        // A near-miss must be a miss: gitnuke destroys what it resolves.
        let worktrees = vec![wt("/repo", Some("main")), wt("/wt/x", Some("issue-421"))];

        assert_eq!(
            resolve_target(&worktrees, "issue-42", None),
            Resolution::NotFound
        );
        assert_eq!(resolve_target(&worktrees, "", None), Resolution::NotFound);
    }

    #[test]
    fn reports_ambiguity_between_a_directory_name_and_a_branch_name() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/shared", Some("branch-a")),
            wt("/wt/other", Some("shared")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "shared", None),
            Resolution::Multiple(vec![1, 2])
        );
    }

    #[test]
    fn resolves_multibyte_directory_and_branch_names() {
        let worktrees = vec![
            wt("/repo", Some("main")),
            wt("/wt/日本語テスト", Some("機能/ログイン")),
            wt("/wt/café", Some("🎉-party")),
        ];

        assert_eq!(
            resolve_target(&worktrees, "日本語テスト", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "機能/ログイン", None),
            Resolution::Single(1)
        );
        assert_eq!(
            resolve_target(&worktrees, "café", None),
            Resolution::Single(2)
        );
        assert_eq!(
            resolve_target(&worktrees, "🎉-party", None),
            Resolution::Single(2)
        );
    }

    #[test]
    fn extracts_only_gitlink_entries_from_ls_files_output() {
        // `<mode> <object> <stage>\t<path>`, NUL-separated.
        let stdout = "100644 aaa 0\tREADME.md\x00160000 bbb 0\tsub\x00\
                      100755 ccc 0\tscript.sh\x00160000 ddd 0\tvendor/lib\x00";

        assert_eq!(
            parse_gitlink_paths(stdout),
            vec!["sub".to_string(), "vendor/lib".to_string()]
        );
    }

    #[test]
    fn extracts_gitlink_paths_with_multibyte_and_space_characters() {
        // `-z` output is unquoted, so these arrive verbatim.
        let stdout = "160000 aaa 0\t日本語/サブ\x00160000 bbb 0\tmy submodule\x00";

        assert_eq!(
            parse_gitlink_paths(stdout),
            vec!["日本語/サブ".to_string(), "my submodule".to_string()]
        );
    }

    #[test]
    fn ignores_entries_that_only_look_like_gitlinks() {
        // A path *named* like a mode, and a regular file whose mode merely
        // starts with the same digits, must not be mistaken for submodules.
        let stdout = "100644 aaa 0\t160000 notes.txt\x001600000 bbb 0\tweird\x00";

        assert!(parse_gitlink_paths(stdout).is_empty());
    }

    #[test]
    fn empty_gitlink_output_yields_nothing() {
        assert!(parse_gitlink_paths("").is_empty());
    }

    #[test]
    fn report_blocks_removal_for_checked_out_submodules_or_metadata() {
        let clean = SubmoduleReport::default();
        assert!(!clean.blocks_removal());

        let with_paths = SubmoduleReport {
            paths: vec!["sub".to_string(), "vendor/lib".to_string()],
            has_module_metadata: false,
        };
        assert!(with_paths.blocks_removal());
        assert_eq!(with_paths.describe(), "submodules (sub, vendor/lib)");

        // git refuses on leftover metadata even with nothing checked out, so
        // gitnuke has to gate on it too or it would gate then fail anyway.
        let metadata_only = SubmoduleReport {
            paths: Vec::new(),
            has_module_metadata: true,
        };
        assert!(metadata_only.blocks_removal());
        assert_eq!(metadata_only.describe(), "submodule metadata");
    }

    #[test]
    fn not_found_message_lists_known_worktrees() {
        let worktrees = vec![wt("/repo", Some("main")), wt("/wt/x", None)];

        let message = not_found_message(&worktrees, "nope");

        assert!(message.contains("no worktree matches 'nope'"));
        assert!(message.contains("/repo [main]"));
        assert!(message.contains("/wt/x [detached HEAD]"));
    }
}
