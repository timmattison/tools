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
}



fn find_git_repository(start_path: Option<&str>) -> Result<Repository> {
    let start = if let Some(path) = start_path {
        PathBuf::from(path)
    } else {
        env::current_dir().context("Failed to get current directory")?
    };
    
    // Try to open repository using git2's discovery mechanism
    // This will search up the directory tree for a .git directory
    Repository::discover(&start)
        .with_context(|| format!("No git repository found starting from {:?}", start))
}


fn check_claude_cli() -> Result<()> {
    use std::process::Command;
    
    // Try different possible Claude locations
    let home = env::var("HOME").unwrap_or_default();
    let claude_paths = vec![
        "claude".to_string(),  // In PATH
        format!("{}/.claude/local/claude", home),  // Common install location
        "/usr/local/bin/claude".to_string(),  // Another common location
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
                // Claude CLI is available, now check if it's authenticated by trying a minimal command
                // We'll check authentication when we actually try to use it
                return Ok(());
            } else {
                // Claude CLI had an error even with --version
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
            // Claude CLI is not found in any of the expected locations
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


async fn generate_commit_message(diff: &str) -> Result<String> {
    use std::process::Command;
    
    let prompt = format!(
        "Based on the following git diff, generate a clear and concise commit message. \
        Follow conventional commit format (type: description). \
        The message should explain what was changed and why, not just describe the diff. \
        Keep it under 72 characters for the subject line. \
        Return ONLY the commit message, no explanation or additional text.\n\n{}",
        diff
    );
    
    // Try different possible Claude locations
    let home = env::var("HOME").unwrap_or_default();
    let claude_paths = vec![
        "claude".to_string(),  // In PATH
        format!("{}/.claude/local/claude", home),  // Common install location
        "/usr/local/bin/claude".to_string(),  // Another common location
    ];
    
    let mut output = None;
    
    for path in &claude_paths {
        let result = Command::new(path)
            .args(&["--print", &prompt])
            .output();
            
        if let Ok(o) = result {
            output = Some(o);
            break;
        }
    }
    
    let output = output
        .ok_or_else(|| anyhow::anyhow!("Failed to execute Claude CLI. Make sure Claude Code is installed and you're logged in with 'claude login'"))?;
    
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
    
    Ok(message)
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
        
        println!("\nRewording the most recent commit: {}", &head_hash[..8]);
        println!("Original message: {}", head.message().unwrap_or(""));
        
        // Get the diff of the HEAD commit
        let commit_diff = get_commit_diff(&repo, &head_hash)?;
        
        println!("\nGenerating new commit message...");
        let new_message = generate_commit_message(&commit_diff).await?;
        
        println!("\nGenerated commit message:");
        println!("{}", new_message);
        
        if !args.dry_run {
            use std::process::Command;
            
            println!("\nAmending commit...");
            
            // Use git commit --amend to update the message
            let output = Command::new("git")
                .args(&["commit", "--amend", "-m", &new_message])
                .output()
                .context("Failed to amend commit")?;
            
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Failed to amend commit: {}", error);
            }
            
            println!("Commit successfully reworded!");
            println!("\nWARNING: The commit hash has changed. If you've already pushed this commit,");
            println!("you'll need to force push with: git push --force-with-lease");
        }
    } else if let Some(commit_hash) = args.fixup {
        // Handle fixup mode
        println!("\nWARNING: Using --fixup will create a fixup commit that will:");
        println!("- Change the commit message of commit {}", commit_hash);
        println!("- Require running 'git rebase -i --autosquash' to apply the change");
        println!("- Change all commit hashes after the target commit\n");
        
        // Get the diff of the commit to reword
        let commit_diff = get_commit_diff(&repo, &commit_hash)?;
        
        println!("Generating new commit message for commit {}...", commit_hash);
        let new_message = generate_commit_message(&commit_diff).await?;
        
        println!("\nGenerated commit message:");
        println!("{}", new_message);
        
        if !args.dry_run {
            use std::process::Command;
            
            println!("\nCreating fixup commit...");
            
            // Create the fixup commit using git command
            let output = Command::new("git")
                .args(&["commit", "--allow-empty", &format!("--fixup=reword:{}:{}", commit_hash, new_message)])
                .output()
                .context("Failed to create fixup commit")?;
            
            if !output.status.success() {
                let error = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Failed to create fixup commit: {}", error);
            }
            
            println!("Fixup commit created successfully!");
            println!("\nTo apply this change, run:");
            println!("  git rebase -i --autosquash {}", commit_hash);
            println!("\nOr to automatically apply it:");
            println!("  git rebase -i --autosquash {}~1", commit_hash);
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
        let commit_message = generate_commit_message(&staged_diff).await?;
        
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