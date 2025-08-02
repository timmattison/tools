use anyhow::{Context, Result};
use clap::Parser;
use git2::{DiffOptions, Oid, Repository};
use std::env;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about = "Automatically generate git commit messages using Claude")]
struct Args {
    #[arg(short, long, help = "Path to repository (defaults to current directory)")]
    path: Option<String>,
    
    #[arg(short, long, help = "Don't actually create the commit, just show the message")]
    dry_run: bool,
    
    #[arg(short, long, help = "Stage all modified files before committing")]
    all: bool,
    
    #[arg(short, long, help = "Reword a previous commit (provide commit hash)")]
    fixup: Option<String>,
    
    #[arg(short, long, help = "Reword the most recent commit")]
    reword: bool,
    
    #[arg(long, help = "Generate a longer, more detailed commit message with context")]
    long: bool,
}

fn find_git_repository(start_path: Option<&str>) -> Result<Repository> {
    let start = if let Some(path) = start_path {
        PathBuf::from(path)
    } else {
        env::current_dir().context("Failed to get current directory")?
    };
    
    Repository::discover(&start)
        .with_context(|| format!("No git repository found starting from {:?}", start))
}

fn check_claude_cli() -> Result<()> {
    use std::process::Command;
    
    let home = env::var("HOME").unwrap_or_default();
    let claude_paths = vec![
        "claude".to_string(),
        format!("{}/.claude/local/claude", home),
        "/usr/local/bin/claude".to_string(),
    ];
    
    let mut claude_check = None;
    let mut used_path = String::new();
    
    for path in &claude_paths {
        let result = Command::new(path)
            .arg("--version")
            .output();
            
        if result.is_ok() {
            claude_check = Some(result);
            used_path = path.clone();
            break;
        }
    }
    
    match claude_check {
        Some(Ok(output)) => {
            if output.status.success() {
                return Ok(());
            } else {
                let error_msg = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!(
                    "Claude Code encountered an error: {}\n\n\
                    Try reinstalling Claude Code from https://claude.ai/code",
                    error_msg.trim()
                )
            }
        },
        Some(Err(e)) => {
            anyhow::bail!("Failed to run Claude CLI at {}: {}", used_path, e)
        },
        None => {
            anyhow::bail!(
                "Claude Code is not installed or not in expected locations.\n\n\
                Checked locations:\n\
                - claude (in PATH)\n\
                - ~/.claude/local/claude\n\
                - /usr/local/bin/claude\n\n\
                To use inscribe with your Claude.ai subscription:\n\
                1. Install Claude Code from: https://claude.ai/code\n\
                2. Run 'claude login' to authenticate\n\
                3. Then run inscribe again"
            )
        }
    }
}

fn get_diff(repo: &Repository, staged: bool) -> Result<String> {
    let mut diff_options = DiffOptions::new();
    
    let diff = if staged {
        let head = repo.head()?.peel_to_tree()?;
        let mut index = repo.index()?;
        let oid = index.write_tree()?;
        let index_tree = repo.find_tree(oid)?;
        repo.diff_tree_to_tree(Some(&head), Some(&index_tree), Some(&mut diff_options))?
    } else {
        let head = repo.head()?.peel_to_tree()?;
        repo.diff_tree_to_workdir_with_index(Some(&head), Some(&mut diff_options))?
    };
    
    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            diff_text.push_str(content);
        }
        true
    })?;
    
    Ok(diff_text)
}

fn get_commit_diff(repo: &Repository, commit_hash: &str) -> Result<String> {
    let oid = Oid::from_str(commit_hash)
        .context("Invalid commit hash")?;
    
    let commit = repo.find_commit(oid)
        .context("Commit not found")?;
    
    let commit_tree = commit.tree()?;
    
    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };
    
    let mut diff_options = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), Some(&mut diff_options))?;
    
    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            diff_text.push_str(content);
        }
        true
    })?;
    
    Ok(diff_text)
}

async fn generate_commit_message(diff: &str, long_format: bool) -> Result<String> {
    use std::process::{Command, Stdio};
    use std::io::Write;
    use tokio::time::{timeout, Duration};
    
    let prompt = if long_format {
        format!(
            "Based on the following git diff, generate a detailed commit message with: \
            1. A clear subject line under 72 characters following conventional commit format (type: description) \
            2. A blank line \
            3. A detailed body explaining: \
               - What was changed and why \
               - Any important context or implications \
               - Any breaking changes or considerations \
            The body should wrap at 72 characters per line. \
            Return ONLY the commit message (subject and body), no explanation or additional text.\n\n{}",
            diff
        )
    } else {
        format!(
            "Based on the following git diff, generate a clear and concise commit message. \
            Follow conventional commit format (type: description). \
            The message should explain what was changed and why, not just describe the diff. \
            Keep it under 72 characters for the subject line. \
            Return ONLY the commit message, no explanation or additional text.\n\n{}",
            diff
        )
    };
    
    // If diff is very large, truncate it to avoid issues
    let truncated_prompt = if prompt.len() > 10000 {
        let truncated_diff = &diff[..8000];
        if long_format {
            format!(
                "Based on the following git diff, generate a detailed commit message with: \
                1. A clear subject line under 72 characters following conventional commit format (type: description) \
                2. A blank line \
                3. A detailed body explaining: \
                   - What was changed and why \
                   - Any important context or implications \
                   - Any breaking changes or considerations \
                The body should wrap at 72 characters per line. \
                Return ONLY the commit message (subject and body), no explanation or additional text.\n\n{}\n\n[... diff truncated for length ...]",
                truncated_diff
            )
        } else {
            format!(
                "Based on the following git diff, generate a clear and concise commit message. \
                Follow conventional commit format (type: description). \
                The message should explain what was changed and why, not just describe the diff. \
                Keep it under 72 characters for the subject line. \
                Return ONLY the commit message, no explanation or additional text.\n\n{}\n\n[... diff truncated for length ...]",
                truncated_diff
            )
        }
    } else {
        prompt
    };
    
    let home = env::var("HOME").unwrap_or_default();
    let claude_paths = vec![
        "claude".to_string(),
        format!("{}/.claude/local/claude", home),
        "/usr/local/bin/claude".to_string(),
    ];
    
    let mut claude_path = None;
    for path in &claude_paths {
        if std::fs::metadata(path).is_ok() {
            claude_path = Some(path);
            break;
        }
    }
    
    let claude_path = claude_path
        .ok_or_else(|| anyhow::anyhow!("Claude CLI not found. Make sure Claude Code is installed."))?;
    
    // Use stdin for the prompt to handle large diffs better
    let mut child = Command::new(claude_path)
        .arg("--print")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn Claude CLI")?;
    
    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(truncated_prompt.as_bytes())
            .context("Failed to write prompt to Claude CLI stdin")?;
    }
    
    // Wait for output with timeout
    let output = tokio::task::spawn_blocking(move || {
        child.wait_with_output()
    });
    
    let output = timeout(Duration::from_secs(30), output)
        .await
        .context("Claude CLI timed out after 30 seconds")?
        .context("Failed to join Claude CLI task")?
        .context("Failed to wait for Claude CLI output")?;
    
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("not authenticated") || error.contains("login") {
            anyhow::bail!(
                "Claude Code is not authenticated.\n\n\
                Please run: claude login\n\
                Then authenticate with your Claude.ai account."
            );
        }
        anyhow::bail!("Claude CLI error: {}", error);
    }
    
    let message = String::from_utf8(output.stdout)?
        .trim()
        .to_string();
    
    if message.is_empty() {
        anyhow::bail!("Claude CLI returned empty message");
    }
    
    Ok(message)
}

fn amend_commit_with_git2(repo: &Repository, new_message: &str) -> Result<()> {
    // Get HEAD commit
    let head = repo.head()?.peel_to_commit()?;
    
    // Get the author and committer signatures
    let author = head.author();
    let committer = repo.signature()?;
    
    // Amend the commit with the new message
    let amended_commit = head.amend(
        Some("HEAD"),           // update_ref
        Some(&author),          // author (None keeps original)
        Some(&committer),       // committer (None keeps original)
        None,                   // message_encoding (None for UTF-8)
        Some(new_message),      // new message
        None                    // tree (None keeps original tree)
    )?;
    
    println!("Commit successfully reworded! New commit: {}", amended_commit);
    
    Ok(())
}

fn reword_commit_with_rebase(repo: &Repository, commit_hash: &str, new_message: &str) -> Result<()> {
    use git2::RebaseOptions;
    
    let target_oid = Oid::from_str(commit_hash)?;
    let target_commit = repo.find_commit(target_oid)?;
    
    // Find the parent of the target commit
    if target_commit.parent_count() == 0 {
        anyhow::bail!("Cannot reword root commit with rebase");
    }
    
    let parent_commit = target_commit.parent(0)?;
    let parent_annotated = repo.find_annotated_commit(parent_commit.id())?;
    
    // Get the current branch reference
    let head = repo.head()?;
    let branch_annotated = repo.reference_to_annotated_commit(&head)?;
    
    // Create rebase options
    let mut rebase_options = RebaseOptions::new();
    rebase_options.quiet(true);
    
    // Start the rebase from the parent of the commit we want to reword
    let mut rebase = repo.rebase(
        Some(&branch_annotated),  // branch (current branch)
        Some(&parent_annotated),  // upstream (rebase onto parent of target)
        None,                     // onto (use upstream)
        Some(&mut rebase_options)
    )?;
    
    let signature = repo.signature()?;
    
    // Process each commit in the rebase
    while let Some(operation) = rebase.next() {
        let operation = operation?;
        let operation_id = operation.id();
        
        // Check if this is the commit we want to reword
        if operation_id == target_oid {
            // Use the new message for this commit
            rebase.commit(None, &signature, Some(new_message))?;
        } else {
            // Keep the original message for other commits
            rebase.commit(None, &signature, None)?;
        }
    }
    
    // Finish the rebase
    rebase.finish(Some(&signature))?;
    
    println!("Commit message successfully updated!");
    println!("\nWARNING: All commit hashes after {} have changed.", &commit_hash[..commit_hash.len().min(8)]);
    println!("If you've already pushed, you'll need to force push.");
    
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    // Check if Claude CLI is available
    check_claude_cli()?;
    
    let repo = find_git_repository(args.path.as_deref())?;
    
    if args.reword {
        // Handle reword mode for the most recent commit
        let head = repo.head()?.peel_to_commit()?;
        let head_oid = head.id();
        let head_hash = head_oid.to_string();
        
        println!("\nRewording the most recent commit: {}", &head_hash[..head_hash.len().min(8)]);
        println!("Original message: {}", head.message().unwrap_or(""));
        
        // Get the diff of the HEAD commit
        let commit_diff = get_commit_diff(&repo, &head_hash)?;
        
        println!("\nGenerating new commit message...");
        let new_message = generate_commit_message(&commit_diff, args.long).await?;
        
        println!("\nGenerated commit message:");
        println!("{}", new_message);
        
        if !args.dry_run {
            println!("\nAmending commit...");
            
            // Use git2 to amend the commit
            amend_commit_with_git2(&repo, &new_message)?;
            
            println!("\nWARNING: The commit hash has changed. If you've already pushed this commit,");
            println!("you'll need to force push with: git push --force-with-lease");
        }
    } else if let Some(commit_hash) = args.fixup {
        // Handle fixup mode
        println!("\nRewording commit {}...", &commit_hash[..commit_hash.len().min(8)]);
        println!("This will:");
        println!("- Generate a new commit message using Claude");
        println!("- Update the commit message directly");
        println!("- Change all commit hashes after the target commit\n");
        
        // Get the diff of the commit to reword
        let commit_diff = get_commit_diff(&repo, &commit_hash)?;
        
        println!("Generating new commit message for commit {}...", commit_hash);
        let new_message = generate_commit_message(&commit_diff, args.long).await?;
        
        println!("\nGenerated commit message:");
        println!("{}", new_message);
        
        if !args.dry_run {
            println!("\nApplying new commit message...");
            
            // Use git2 rebase to reword the commit
            reword_commit_with_rebase(&repo, &commit_hash, &new_message)?;
        }
    } else {
        // Normal commit mode
        if args.all {
            let mut index = repo.index()?;
            index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
            index.write()?;
        }
        
        let staged_diff = get_diff(&repo, true)?;
        if staged_diff.is_empty() {
            anyhow::bail!("No staged changes found. Use -a to stage all changes.");
        }
        
        println!("Generating commit message...");
        let commit_message = generate_commit_message(&staged_diff, args.long).await?;
        
        println!("\nGenerated commit message:");
        println!("{}", commit_message);
        
        if !args.dry_run {
            println!("\nCreating commit...");
            
            let signature = repo.signature()?;
            let tree_oid = repo.index()?.write_tree()?;
            let tree = repo.find_tree(tree_oid)?;
            let parent_commit = repo.head()?.peel_to_commit()?;
            
            repo.commit(
                Some("HEAD"),
                &signature,
                &signature,
                &commit_message,
                &tree,
                &[&parent_commit],
            )?;
            
            println!("Commit created successfully!");
        }
    }
    
    Ok(())
}