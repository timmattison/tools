use anyhow::{Context, Result};
use clap::{Parser, ValueHint};
use colored::Colorize;
use crossbeam::channel::{bounded, select, Sender};
use git2::{Branch, BranchType, Commit, ObjectType, Repository, Sort};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicI32, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};
use walkdir::{DirEntry, WalkDir};

/// Command line arguments
#[derive(Parser, Debug)]
#[command(
    name = "gitdiggin-rs",
    author = "Tim Mattison",
    about = "Recursively searches Git repositories for commits containing a specific string",
    version,
    long_about = None,
    after_help = "Examples:
  gitdiggin-rs registration               # Search for 'registration' in commit messages
  gitdiggin-rs --contents api /path/to/dir # Search for 'api' in messages and contents
  gitdiggin-rs --all fix                  # Search for 'fix' in all branches"
)]
struct Args {
    /// Search term to look for in commit messages and optionally contents
    #[arg(required = true, value_hint = ValueHint::Other)]
    search_term: String,

    /// Root directory to start scanning from (overrides positional arguments)
    #[arg(long, value_hint = ValueHint::DirPath)]
    root: Option<PathBuf>,

    /// Search in commit contents (diffs) in addition to commit messages
    #[arg(long)]
    contents: bool,

    /// Search all branches, not just the current branch
    #[arg(long)]
    all: bool,

    /// Suppress output about directories that couldn't be accessed
    #[arg(long = "ignore-failures")]
    ignore_failures: bool,

    /// Search paths
    #[arg(value_hint = ValueHint::DirPath)]
    paths: Vec<PathBuf>,
}

/// Directories to skip while walking the filesystem
const SKIP_DIRS: [&str; 6] = [
    "node_modules", "vendor", ".idea", ".vscode", "dist", "build",
];

/// Represents a search result
struct SearchResult {
    repositories: HashMap<String, Vec<String>>,
    inaccessible_dirs: Vec<String>,
    found_commits: bool,
    abs_paths: Vec<String>,
    search_term: String,
    search_contents: bool,
}

/// Progress information to display
struct ProgressInfo {
    dirs_checked: i32,
    repos_found: i32,
    current_path: String,
}

fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();
    
    // Determine search paths
    let paths = if let Some(root) = args.root.as_ref() {
        vec![root.clone()]
    } else if !args.paths.is_empty() {
        args.paths.clone()
    } else {
        vec![PathBuf::from(".")]
    };

    // Set up progress indicators
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );

    // Display initial message
    println!(
        "🔍 Searching for \"{}\" in commit {}",
        args.search_term,
        if args.contents {
            "messages and contents"
        } else {
            "messages"
        }
    );
    println!("{}", "─".repeat(50));
    
    // Set up shared state
    let dirs_checked = Arc::new(AtomicI32::new(0));
    let repos_found = Arc::new(AtomicI32::new(0));
    let current_path = Arc::new(Mutex::new(String::new()));
    let search_result = Arc::new(Mutex::new(SearchResult {
        repositories: HashMap::new(),
        inaccessible_dirs: Vec::new(),
        found_commits: false,
        abs_paths: Vec::new(),
        search_term: args.search_term.clone(),
        search_contents: args.contents,
    }));
    let searched_repos = Arc::new(Mutex::new(HashSet::new()));
    let running = Arc::new(AtomicBool::new(true));
    
    // Set up channels for progress updates
    let (tx, rx) = bounded::<ProgressInfo>(100);
    
    // Resolve and store absolute paths
    let abs_paths = paths
        .iter()
        .filter_map(|p| {
            let path_str = p.to_string_lossy().to_string();
            match p.canonicalize() {
                Ok(abs_path) => Some(abs_path.to_string_lossy().to_string()),
                Err(err) => {
                    if !args.ignore_failures {
                        let mut result = search_result.lock().unwrap();
                        result.inaccessible_dirs.push(format!(
                            "{} (could not resolve absolute path: {})",
                            path_str, err
                        ));
                    }
                    None
                }
            }
        })
        .collect::<Vec<String>>();
    
    {
        let mut result = search_result.lock().unwrap();
        result.abs_paths = abs_paths.clone();
    }
    
    // Set up progress monitoring thread
    let running_clone = running.clone();
    let pb_clone = pb.clone();
    let dirs_checked_clone = dirs_checked.clone();
    let repos_found_clone = repos_found.clone();
    let start_time = Instant::now();
    
    let progress_thread = thread::spawn(move || {
        while running_clone.load(Ordering::Relaxed) {
            // Update progress display
            select! {
                recv(rx) -> msg => {
                    if let Ok(progress) = msg {
                        pb_clone.set_message(format!(
                            "Directories scanned: {} | Repositories found: {} | Current: {}",
                            progress.dirs_checked,
                            progress.repos_found,
                            if progress.current_path.len() > 38 {
                                format!("...{}", &progress.current_path[progress.current_path.len() - 35..])
                            } else {
                                progress.current_path
                            }
                        ));
                    }
                },
                default(Duration::from_millis(100)) => {
                    // Update stats periodically even if no new messages
                    let elapsed = start_time.elapsed();
                    let dirs = dirs_checked_clone.load(Ordering::Relaxed);
                    let repos = repos_found_clone.load(Ordering::Relaxed);
                    
                    let scan_rate = if elapsed.as_secs() > 0 {
                        dirs as f64 / elapsed.as_secs() as f64
                    } else {
                        0.0
                    };
                    
                    pb_clone.set_message(format!(
                        "Dirs: {} | Repos: {} | Rate: {:.1} dirs/sec | Time: {:?}",
                        dirs, repos, scan_rate, elapsed
                    ));
                }
            }
        }
        pb_clone.finish_and_clear();
    });

    // Create worker threads for scanning
    let handles = abs_paths
        .iter()
        .map(|path| {
            let path = path.clone();
            let search_result = search_result.clone();
            let dirs_checked = dirs_checked.clone();
            let repos_found = repos_found.clone();
            let current_path = current_path.clone();
            let searched_repos = searched_repos.clone();
            let search_term = args.search_term.clone();
            let tx = tx.clone();
            let ignore_failures = args.ignore_failures;
            let search_contents = args.contents;
            let search_all_branches = args.all;

            thread::spawn(move || {
                scan_path(
                    &path,
                    search_result,
                    dirs_checked,
                    repos_found,
                    current_path,
                    searched_repos,
                    tx,
                    ignore_failures,
                    &search_term,
                    search_contents,
                    search_all_branches,
                )
                .unwrap_or_else(|err| {
                    if !ignore_failures {
                        let mut result = search_result.lock().unwrap();
                        result
                            .inaccessible_dirs
                            .push(format!("{} (walk error: {})", path, err));
                    }
                });
            })
        })
        .collect::<Vec<_>>();

    // Wait for scanning to complete
    for handle in handles {
        handle.join().unwrap();
    }
    
    // Signal progress thread to exit
    running.store(false, Ordering::Relaxed);
    progress_thread.join().unwrap();

    // Print results
    let result = search_result.lock().unwrap();
    
    // Print inaccessible directories
    if !args.ignore_failures && !result.inaccessible_dirs.is_empty() {
        println!("\n⚠️  The following directories could not be fully accessed:");
        for dir in &result.inaccessible_dirs {
            println!("  {}", dir);
        }
        println!();
    }
    
    // Print search results
    if result.found_commits {
        println!(
            "🔍 Found commits containing \"{}\"",
            result.search_term
        );
        
        if result.search_contents {
            println!("📄 Searched in commit messages and contents");
        } else {
            println!("📄 Searched in commit messages only");
        }
        
        println!(
            "📂 Search paths: {}",
            result.abs_paths.join(", ")
        );
        
        if args.all {
            println!("🔀 Searched across all branches");
        }
        
        println!();
        
        // Calculate total commits
        let total_commits: usize = result.repositories.values().map(|v| v.len()).sum();
        
        println!("📊 Summary:");
        println!(
            "   • Found {} matching commits across {} repositories\n",
            total_commits,
            result.repositories.len()
        );
        
        // Sort repository paths for consistent output
        let mut sorted_repo_paths: Vec<&String> = result.repositories.keys().collect();
        sorted_repo_paths.sort();
        
        // Display results in sorted order
        for working_dir in sorted_repo_paths {
            let commits = &result.repositories[working_dir];
            println!("📁 {} - {} commits", working_dir, commits.len());
            
            for commit in commits {
                println!("      • {}", commit);
            }
            
            println!();
        }
    } else {
        println!("😴 No commits found containing \"{}\"", result.search_term);
        
        if result.search_contents {
            println!("   • Searched in commit messages and contents");
        } else {
            println!("   • Searched in commit messages only");
        }
        
        println!(
            "   • Search paths: {}",
            result.abs_paths.join(", ")
        );
    }
    
    Ok(())
}

/// Check if a directory entry should be skipped
fn is_ignored(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|name| SKIP_DIRS.contains(&name))
        .unwrap_or(false)
}

/// Scan a path for git repositories
fn scan_path(
    path: &str,
    search_result: Arc<Mutex<SearchResult>>,
    dirs_checked: Arc<AtomicI32>,
    repos_found: Arc<AtomicI32>,
    current_path: Arc<Mutex<String>>,
    searched_repos: Arc<Mutex<HashSet<String>>>,
    tx: Sender<ProgressInfo>,
    ignore_failures: bool,
    search_term: &str,
    search_contents: bool,
    search_all_branches: bool,
) -> Result<()> {
    // Check if path exists
    if !Path::new(path).exists() {
        if !ignore_failures {
            let mut result = search_result.lock().unwrap();
            result
                .inaccessible_dirs
                .push(format!("{} (access error: path does not exist)", path));
        }
        return Ok(());
    }

    // Walk the directory tree
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_ignored(e))
    {
        match entry {
            Ok(entry) => {
                // Skip non-directories
                if !entry.file_type().is_dir() {
                    continue;
                }

                let entry_path = entry.path().to_string_lossy().to_string();
                
                // Update progress
                dirs_checked.fetch_add(1, Ordering::Relaxed);
                {
                    let mut path = current_path.lock().unwrap();
                    *path = entry_path.clone();
                }
                
                // Send progress update
                let _ = tx.send(ProgressInfo {
                    dirs_checked: dirs_checked.load(Ordering::Relaxed),
                    repos_found: repos_found.load(Ordering::Relaxed),
                    current_path: entry_path.clone(),
                });
                
                // Check if it's a git repository (has .git directory)
                let git_dir = entry.path().join(".git");
                if git_dir.exists() && git_dir.is_dir() {
                    // Ensure we haven't processed this repository before
                    let canonical_path = match entry.path().canonicalize() {
                        Ok(path) => path.to_string_lossy().to_string(),
                        Err(_) => entry_path.clone(),
                    };
                    
                    let mut repo_set = searched_repos.lock().unwrap();
                    if repo_set.contains(&canonical_path) {
                        continue;
                    }
                    
                    repo_set.insert(canonical_path);
                    repos_found.fetch_add(1, Ordering::Relaxed);
                    
                    // Send progress update
                    let _ = tx.send(ProgressInfo {
                        dirs_checked: dirs_checked.load(Ordering::Relaxed),
                        repos_found: repos_found.load(Ordering::Relaxed),
                        current_path: entry_path.clone(),
                    });
                    
                    // Process git repository
                    process_git_repo(
                        &entry_path,
                        search_result.clone(),
                        ignore_failures,
                        search_term,
                        search_contents,
                        search_all_branches,
                    );
                }
            }
            Err(err) => {
                if !ignore_failures {
                    let mut result = search_result.lock().unwrap();
                    result.inaccessible_dirs.push(format!(
                        "{} (access error: {})",
                        err.path().unwrap_or(Path::new("unknown")).to_string_lossy(),
                        err
                    ));
                }
            }
        }
    }
    
    Ok(())
}

/// Process a git repository to find commits matching the search term
fn process_git_repo(
    path: &str,
    search_result: Arc<Mutex<SearchResult>>,
    ignore_failures: bool,
    search_term: &str,
    search_contents: bool,
    search_all_branches: bool,
) {
    // Open the repository
    let repo = match Repository::open(path) {
        Ok(repo) => repo,
        Err(err) => {
            if !ignore_failures {
                let mut result = search_result.lock().unwrap();
                result.inaccessible_dirs.push(format!(
                    "{} (error opening git repo: {})",
                    path, err
                ));
            }
            return;
        }
    };

    let mut matching_commits = Vec::new();

    // Process a single branch reference
    let process_branch = |branch: Result<Branch, git2::Error>| -> Result<(), git2::Error> {
        let branch = branch?;
        let commit = branch.get().peel_to_commit()?;
        
        // Create a revwalk to iterate through commits
        let mut revwalk = repo.revwalk()?;
        revwalk.push(commit.id())?;
        revwalk.set_sorting(Sort::TIME)?;
        
        for oid in revwalk {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            let message = commit.message().unwrap_or("");
            
            // Check if commit message contains the search term
            if message.to_lowercase().contains(&search_term.to_lowercase()) {
                add_matching_commit(&mut matching_commits, &commit, message);
                continue;
            }
            
            // If not searching contents, skip to next commit
            if !search_contents {
                continue;
            }
            
            // Get parent to compare changes
            if commit.parent_count() == 0 {
                continue;
            }
            
            let parent = match commit.parent(0) {
                Ok(parent) => parent,
                Err(_) => continue,
            };
            
            // Get the diff between this commit and its parent
            let parent_tree = match parent.tree() {
                Ok(tree) => tree,
                Err(_) => continue,
            };
            
            let commit_tree = match commit.tree() {
                Ok(tree) => tree,
                Err(_) => continue,
            };
            
            let diff = match repo.diff_tree_to_tree(
                Some(&parent_tree),
                Some(&commit_tree),
                None,
            ) {
                Ok(diff) => diff,
                Err(_) => continue,
            };
            
            // Search for term in the diff content
            let mut found_in_diff = false;
            diff.foreach(
                &mut |_, _| true,
                None,
                Some(&mut |_, hunk| {
                    if let Some(content) = hunk.content() {
                        if content.to_lowercase().contains(&search_term.to_lowercase()) {
                            found_in_diff = true;
                            return false; // Stop iterating
                        }
                    }
                    true
                }),
                None,
            ).unwrap_or(());
            
            if found_in_diff {
                add_matching_commit(&mut matching_commits, &commit, message);
            }
        }
        
        Ok(())
    };

    let result = if search_all_branches {
        // Search all branches
        let branches = match repo.branches(Some(BranchType::Local)) {
            Ok(branches) => branches,
            Err(err) => {
                if !ignore_failures {
                    let mut result = search_result.lock().unwrap();
                    result.inaccessible_dirs.push(format!(
                        "{} (error getting branches: {})",
                        path, err
                    ));
                }
                return;
            }
        };
        
        for branch in branches {
            if let Err(err) = process_branch(branch) {
                if !ignore_failures {
                    let mut result = search_result.lock().unwrap();
                    result.inaccessible_dirs.push(format!(
                        "{} (error processing branch: {})",
                        path, err
                    ));
                }
            }
        }
    } else {
        // Just use HEAD
        match repo.head() {
            Ok(head_ref) => {
                if let Ok(branch) = Branch::wrap(head_ref) {
                    if let Err(err) = process_branch(Ok(branch)) {
                        if !ignore_failures {
                            let mut result = search_result.lock().unwrap();
                            result.inaccessible_dirs.push(format!(
                                "{} (error processing HEAD: {})",
                                path, err
                            ));
                        }
                    }
                }
            }
            Err(err) => {
                if !ignore_failures {
                    let mut result = search_result.lock().unwrap();
                    result.inaccessible_dirs.push(format!(
                        "{} (error getting HEAD: {})",
                        path, err
                    ));
                }
            }
        }
    };
    
    // If we found any matching commits, update the search results
    if !matching_commits.is_empty() {
        let mut result = search_result.lock().unwrap();
        result.found_commits = true;
        result.repositories.insert(path.to_string(), matching_commits);
    }
}

/// Add a matching commit to the results
fn add_matching_commit(matching_commits: &mut Vec<String>, commit: &Commit, message: &str) {
    // Get the first line of the commit message
    let first_line = message
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    
    // Format the commit info
    matching_commits.push(format!(
        "{} {}",
        commit.id().to_string(),
        first_line
    ));
}