use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, TimeZone};
use git2::{Repository, BranchType, Commit, Oid, Time};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use walkdir::WalkDir;

use crate::stats::{GitStats, Timer};

/// Progress callback function type
pub type ProgressCallback = dyn Fn(usize, usize, &str) + Send + Sync;

/// Options for scanning repositories
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Whether to search all branches
    pub search_all_branches: bool,
    /// Whether to filter commits by user email
    pub filter_by_user: bool,
    /// Whether to find nested repositories
    pub find_nested: bool,
    /// Whether to ignore access failures
    pub ignore_failures: bool,
}

/// Progress tracking for repository scanning
pub struct ScanProgress<'a> {
    /// Counter for directories checked
    pub dirs_checked: &'a Arc<AtomicUsize>,
    /// Counter for repositories found
    pub repos_found: &'a Arc<AtomicUsize>,
    /// Optional callback for progress updates
    pub progress_callback: Option<&'a Arc<ProgressCallback>>,
}

/// Options for processing a git repository
#[derive(Debug, Clone)]
pub struct RepoProcessOptions {
    /// Whether to search all branches
    pub search_all_branches: bool,
    /// Whether to filter commits by user email
    pub filter_by_user: bool,
}

/// Directories to skip while walking the filesystem
const SKIP_DIRS: &[&str] = &[
    "node_modules", "vendor", ".idea", ".vscode", "dist", "build",
];

/// Check if a directory is a Git repository
///
/// # Errors
///
/// This function returns `Ok(bool)` and does not error.
pub fn is_git_repository(path: &Path, stats: &GitStats) -> Result<bool> {
    let timer = Timer::new();
    let result = Repository::open(path).is_ok();
    stats.record_git_dir(timer.elapsed());
    Ok(result)
}

/// Get the current git user email
///
/// # Errors
///
/// Returns an error if the git config cannot be read or user.email is not set.
pub fn get_git_user_email(stats: &GitStats) -> Result<String> {
    let timer = Timer::new();
    
    // Try to get from global git config
    let config = git2::Config::open_default()?;
    let email = config.get_string("user.email")?;
    
    stats.record_email(timer.elapsed());
    Ok(email)
}

/// Get the root directory of the git repository containing the given path
pub fn get_repository_root(path: &Path) -> Option<PathBuf> {
    match Repository::discover(path) {
        Ok(repo) => {
            repo.workdir().map(|p| p.to_path_buf())
        }
        Err(_) => None,
    }
}

/// Convert git2::Time to DateTime<Local>
fn git_time_to_datetime(time: &Time) -> DateTime<Local> {
    Local.timestamp_opt(time.seconds(), 0).unwrap()
}

/// Get the current branch name of a repository
pub fn get_current_branch(repo_path: &Path) -> String {
    if let Ok(repo) = Repository::open(repo_path) {
        if let Ok(head) = repo.head() {
            if let Some(branch_name) = head.shorthand() {
                return branch_name.to_string();
            }
        }
    }
    "HEAD".to_string() // Default fallback
}

/// Get the first line of a commit message
fn get_first_line(message: &str) -> &str {
    message.lines().next().unwrap_or("")
}

/// Commit information
#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub date: DateTime<Local>,
    pub full_message: String,
}

impl CommitInfo {
    fn from_commit(commit: &Commit) -> Self {
        let author = commit.author();
        Self {
            hash: commit.id().to_string(),
            message: format!("{} {}", commit.id(), get_first_line(commit.message().unwrap_or(""))),
            author_name: author.name().unwrap_or("").to_string(),
            author_email: author.email().unwrap_or("").to_string(),
            date: git_time_to_datetime(&author.when()),
            full_message: commit.message().unwrap_or("").to_string(),
        }
    }
}

/// Search results from a repository scan
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub repositories: HashMap<PathBuf, Vec<CommitInfo>>,
    pub inaccessible_dirs: Vec<String>,
    pub found_commits: bool,
    pub abs_paths: Vec<PathBuf>,
    pub threshold: DateTime<Local>,
    pub end_time: Option<DateTime<Local>>,
    pub stats: GitStats,
}

impl SearchResult {
    pub fn new(threshold: DateTime<Local>, end_time: Option<DateTime<Local>>) -> Self {
        Self {
            repositories: HashMap::new(),
            inaccessible_dirs: Vec::new(),
            found_commits: false,
            abs_paths: Vec::new(),
            threshold,
            end_time,
            stats: GitStats::new(),
        }
    }
}

/// Scan a path for Git repositories and collect commits
///
/// # Errors
///
/// Returns an error if directory walking fails or if a repository cannot be processed.
///
/// # Panics
///
/// Panics if the result mutex is poisoned.
pub fn scan_path(
    search_path: &Path,
    result: &Arc<Mutex<SearchResult>>,
    user_email: &str,
    options: &ScanOptions,
    progress: &ScanProgress<'_>,
) -> Result<()> {
    // Add the search path to abs_paths in the result
    if let Ok(mut result_guard) = result.lock() {
        if let Ok(abs_path) = search_path.canonicalize() {
            result_guard.abs_paths.push(abs_path);
        }
    }
    let walker = WalkDir::new(search_path)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            if !e.file_type().is_dir() {
                return true;
            }

            let name = e.file_name().to_str().unwrap_or("");
            !SKIP_DIRS.contains(&name)
        });

    let mut unique_repos = HashSet::new();

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                if !options.ignore_failures {
                    if let Ok(mut result) = result.lock() {
                        result.inaccessible_dirs.push(format!("Walk error: {}", e));
                    }
                }
                continue;
            }
        };

        if !entry.file_type().is_dir() {
            continue;
        }

        let path = entry.path();

        // Increment directories checked counter
        let dirs_count = progress.dirs_checked.fetch_add(1, Ordering::Relaxed) + 1;

        // Call progress callback if provided
        if let Some(callback) = progress.progress_callback {
            let repos_count = progress.repos_found.load(Ordering::Relaxed);
            callback(dirs_count, repos_count, &path.display().to_string());
        }

        // Check if this is a git repository
        let mut result_guard = result.lock().unwrap();
        if is_git_repository(path, &result_guard.stats)? {
            let abs_path = path.canonicalize()?;

            // Skip if we've already processed this repository
            if unique_repos.contains(&abs_path) {
                continue;
            }
            unique_repos.insert(abs_path.clone());

            // Increment repositories found counter
            let repos_count = progress.repos_found.fetch_add(1, Ordering::Relaxed) + 1;

            // Update progress with new repo count
            if let Some(callback) = progress.progress_callback {
                let dirs_count = progress.dirs_checked.load(Ordering::Relaxed);
                callback(dirs_count, repos_count, &path.display().to_string());
            }

            let repo_options = RepoProcessOptions {
                search_all_branches: options.search_all_branches,
                filter_by_user: options.filter_by_user,
            };

            // Process the repository
            match process_git_repo(
                path,
                &result_guard.stats,
                result_guard.threshold,
                result_guard.end_time,
                user_email,
                &repo_options,
            ) {
                Ok(commits) => {
                    if !commits.is_empty() {
                        result_guard.found_commits = true;
                        result_guard.repositories.insert(abs_path, commits);
                    }
                }
                Err(e) => {
                    if !options.ignore_failures {
                        result_guard.inaccessible_dirs.push(format!(
                            "{} (git error: {})", path.display(), e
                        ));
                    }
                }
            }

            // Skip subdirectories unless find_nested is enabled
            if !options.find_nested {
                continue;
            }
        }
    }

    Ok(())
}

/// Process a Git repository and extract commits
fn process_git_repo(
    repo_path: &Path,
    stats: &GitStats,
    threshold: DateTime<Local>,
    end_time: Option<DateTime<Local>>,
    user_email: &str,
    options: &RepoProcessOptions,
) -> Result<Vec<CommitInfo>> {
    let timer = Timer::new();

    let repo = Repository::open(repo_path)?;
    let mut commits = Vec::new();

    if options.search_all_branches {
        // Get all branches
        match repo.branches(Some(BranchType::Local)) {
            Ok(branches) => {
                for branch_result in branches {
                    match branch_result {
                        Ok((branch, _)) => {
                            if let Some(oid) = branch.get().target() {
                                match get_commits_from_oid(
                                    &repo, oid, threshold, end_time, user_email, options.filter_by_user
                                ) {
                                    Ok(branch_commits) => commits.extend(branch_commits),
                                    Err(_) => continue, // Skip this branch if commits can't be read
                                }
                            }
                        }
                        Err(_) => continue, // Skip invalid branches
                    }
                }
            }
            Err(_) => {
                // If we can't get branches, fall back to HEAD
                return process_head_only(&repo, threshold, end_time, user_email, options.filter_by_user, stats);
            }
        }
    } else {
        // Just use HEAD
        match repo.head() {
            Ok(head) => {
                if let Some(oid) = head.target() {
                    match get_commits_from_oid(
                        &repo, oid, threshold, end_time, user_email, options.filter_by_user
                    ) {
                        Ok(head_commits) => commits = head_commits,
                        Err(_) => {
                            // If HEAD commits can't be read, return empty
                            stats.record_log(timer.elapsed());
                            return Ok(Vec::new());
                        }
                    }
                } else {
                    // HEAD has no target (unborn branch), return empty
                    stats.record_log(timer.elapsed());
                    return Ok(Vec::new());
                }
            }
            Err(e) => {
                // Handle specific Git errors
                match e.code() {
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound => {
                        // Repository has no commits or HEAD doesn't exist - this is OK, just empty
                        stats.record_log(timer.elapsed());
                        return Ok(Vec::new());
                    }
                    _ => {
                        // Other errors should be propagated
                        return Err(anyhow!("Git error in {}: {}", repo_path.display(), e));
                    }
                }
            }
        }
    }

    stats.record_log(timer.elapsed());
    Ok(commits)
}

/// Helper function to process HEAD only when branch enumeration fails
fn process_head_only(
    repo: &Repository,
    threshold: DateTime<Local>,
    end_time: Option<DateTime<Local>>,
    user_email: &str,
    filter_by_user: bool,
    _stats: &GitStats,
) -> Result<Vec<CommitInfo>> {
    match repo.head() {
        Ok(head) => {
            if let Some(oid) = head.target() {
                match get_commits_from_oid(
                    repo, oid, threshold, end_time, user_email, filter_by_user
                ) {
                    Ok(commits) => Ok(commits),
                    Err(_) => Ok(Vec::new()),
                }
            } else {
                Ok(Vec::new())
            }
        }
        Err(_) => Ok(Vec::new()), // If HEAD fails, just return empty
    }
}

/// Get commits from a specific OID
fn get_commits_from_oid(
    repo: &Repository,
    oid: Oid,
    threshold: DateTime<Local>,
    end_time: Option<DateTime<Local>>,
    user_email: &str,
    filter_by_user: bool,
) -> Result<Vec<CommitInfo>> {
    let mut revwalk = match repo.revwalk() {
        Ok(rw) => rw,
        Err(e) => return Err(anyhow!("Failed to create revwalk: {}", e)),
    };
    
    if let Err(e) = revwalk.push(oid) {
        return Err(anyhow!("Failed to push OID to revwalk: {}", e));
    }
    
    if let Err(e) = revwalk.set_sorting(git2::Sort::TIME) {
        return Err(anyhow!("Failed to set revwalk sorting: {}", e));
    }

    let mut commits = Vec::new();

    for commit_oid_result in revwalk {
        let commit_oid = match commit_oid_result {
            Ok(oid) => oid,
            Err(_) => continue, // Skip invalid commit OIDs
        };
        
        let commit = match repo.find_commit(commit_oid) {
            Ok(commit) => commit,
            Err(_) => continue, // Skip commits that can't be found
        };
        
        let commit_time = git_time_to_datetime(&commit.author().when());
        
        // Check if commit is within time range
        if commit_time < threshold {
            break; // Commits are sorted by time, so we can break here
        }
        
        if let Some(end) = end_time {
            if commit_time > end {
                continue;
            }
        }

        // Filter by user if requested
        if filter_by_user {
            let author = commit.author();
            let author_email = author.email().unwrap_or("");
            if author_email != user_email {
                continue;
            }
        }

        commits.push(CommitInfo::from_commit(&commit));
    }

    Ok(commits)
}