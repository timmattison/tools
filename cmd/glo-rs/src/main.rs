use anyhow::{Context, Result};
use clap::Parser;
use git2::{Object, ObjectType, Oid, Repository};
use human_bytes::human_bytes;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::exit;
use thiserror::Error;

/// A tool to find large objects in Git repositories.
#[derive(Parser)]
#[clap(name = "glo", about = "Find large objects in Git repositories")]
struct Args {
    /// Path to the Git repository (optional, defaults to current directory)
    #[clap(long, value_name = "PATH")]
    repo: Option<PathBuf>,

    /// Number of largest objects to display
    #[clap(long, default_value = "20")]
    top: usize,
}

/// Information about a Git object
#[derive(Debug)]
struct ObjectInfo {
    hash: String,
    size: u64,
    path: String,
}

#[derive(Error, Debug)]
enum GloError {
    #[error("Git repository not found")]
    RepositoryNotFound,
}

/// Find the Git repository from the current directory by searching up the directory tree.
fn find_git_repo() -> Result<PathBuf> {
    let mut current_dir = std::env::current_dir()?;
    let max_iterations = 50;
    let mut iteration_count = 0;

    loop {
        let git_dir = current_dir.join(".git");
        if git_dir.exists() && git_dir.is_dir() {
            return Ok(current_dir);
        }

        // Go up one level
        if !current_dir.pop() {
            break;
        }

        iteration_count += 1;
        if iteration_count >= max_iterations {
            break;
        }
    }

    Err(GloError::RepositoryNotFound.into())
}

/// Get all blob objects from a Git repository
fn get_all_objects(repo: &Repository) -> Result<Vec<ObjectInfo>> {
    let mut objects = Vec::new();
    let mut seen_objects = HashSet::new();

    // Process each reference (branch, tag, etc.)
    for reference in repo.references()? {
        let reference = reference?;
        
        // Skip non-direct references
        if reference.is_remote() || reference.is_tag() || reference.is_note() {
            continue;
        }

        // Get the target object
        if let Ok(obj) = reference.peel_to_commit() {
            // Create a revwalk to iterate through all commits
            let mut revwalk = repo.revwalk()?;
            revwalk.push(obj.id())?;

            // Process each commit
            for commit_id in revwalk {
                let commit_id = commit_id?;
                if let Ok(commit) = repo.find_commit(commit_id) {
                    // Get the tree for this commit
                    if let Ok(tree) = commit.tree() {
                        // Walk the tree to find all blobs
                        tree.walk(git2::TreeWalkMode::PreOrder, |path, entry| {
                            if entry.kind() == Some(ObjectType::Blob) {
                                let oid = entry.id();
                                
                                // Skip if we've already seen this object
                                if !seen_objects.insert(oid) {
                                    return git2::TreeWalkResult::Skip;
                                }
                                
                                // Try to get the blob object
                                if let Ok(blob) = repo.find_blob(oid) {
                                    let full_path = if path.is_empty() { 
                                        entry.name().unwrap_or("").to_string() 
                                    } else {
                                        format!("{}{}", path, entry.name().unwrap_or(""))
                                    };

                                    objects.push(ObjectInfo {
                                        hash: oid.to_string(),
                                        size: blob.size() as u64,
                                        path: full_path,
                                    });
                                }
                            }
                            git2::TreeWalkResult::Ok
                        })?;
                    }
                }
            }
        }
    }

    Ok(objects)
}

fn main() -> Result<()> {
    // Parse command line arguments
    let args = Args::parse();

    // Get the repository path
    let repo_path = match &args.repo {
        Some(path) => path.clone(),
        None => find_git_repo()
            .context("Could not find Git repository. Use --repo to specify a path")?
    };

    // Open the Git repository
    let repo = Repository::open(&repo_path)
        .with_context(|| format!("Could not open Git repository at {:?}", repo_path))?;

    // If using the default repository, print its path
    if args.repo.is_none() {
        println!("Using Git repository at: {}", repo_path.display());
    }

    // Get all objects in the repository
    let mut objects = get_all_objects(&repo)
        .context("Could not get Git objects")?;

    // Sort objects by size (smallest first)
    objects.sort_by_key(|obj| obj.size);

    // Determine how many objects to display
    let display_count = args.top.min(objects.len());
    if display_count == 0 {
        println!("No objects found in repository");
        return Ok(());
    }

    // Print the objects (largest first, limited by topCount)
    // Start from the end of the slice to get the largest objects
    let start_index = objects.len() - display_count;
    for i in 0..display_count {
        let obj = &objects[start_index + i];
        println!(
            "{} {} {}",
            &obj.hash[0..12],
            human_bytes(obj.size as f64),
            obj.path
        );
    }

    Ok(())
}