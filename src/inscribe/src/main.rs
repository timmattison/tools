use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use git2::{DiffOptions, Oid, Repository};
use indicatif::{ProgressBar, ProgressStyle};
use std::env;
use std::path::PathBuf;

/// Maximum prompt size in bytes before truncating the diff.
const MAX_PROMPT_BYTES: usize = 10_000;
/// Target diff size in bytes when truncating (leaves room for the prompt template).
const TRUNCATED_DIFF_BYTES: usize = 8_000;

/// Truncate a string to at most `max_bytes` bytes, landing on a valid UTF-8
/// char boundary. Returns the full string if it fits.
fn truncate_to_byte_limit(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    #[allow(clippy::string_slice, reason = "safe: end is verified to be a char boundary")]
    &s[..end]
}

/// Return the first 8 characters of a string (e.g. for short commit hashes).
fn short_hash(hash: &str) -> String {
    hash.chars().take(8).collect()
}

/// Strip markdown code block fences (```) from a string.
/// Handles opening fences with optional language identifiers and closing fences.
fn strip_markdown_code_block(s: &str) -> String {
    let mut result = s.to_string();

    if let Some(stripped) = result.strip_prefix("```") {
        // Remove opening backticks and optional language identifier
        result = match stripped.split_once('\n') {
            Some((_, rest)) => rest.to_string(),
            None => stripped.to_string(),
        };
    }

    if let Some(stripped) = result.strip_suffix("```") {
        result = stripped.trim().to_string();
    }

    result
}

#[derive(Parser, Debug)]
#[command(
    author,
    version = version_string!(),
    about = "Automatically generate git commit messages using Claude"
)]
struct Args {
    #[arg(
        short,
        long,
        help = "Path to repository (defaults to current directory)"
    )]
    path: Option<String>,

    #[arg(
        short,
        long,
        help = "Don't actually create the commit, just show the message"
    )]
    dry_run: bool,

    #[arg(short, long, help = "Stage all modified files before committing")]
    all: bool,

    #[arg(short, long, help = "Reword a previous commit (provide commit hash)")]
    fixup: Option<String>,

    #[arg(short, long, help = "Reword the most recent commit")]
    reword: bool,

    #[arg(short, long, help = "Generate a shorter, more concise commit message")]
    short: bool,
}

fn find_git_repository(start_path: Option<&str>) -> Result<Repository> {
    let start = if let Some(path) = start_path {
        PathBuf::from(path)
    } else {
        env::current_dir().context("Failed to get current directory")?
    };

    Repository::discover(&start)
        .with_context(|| "Not a git repository (or any parent up to root). inscribe must be run inside a git repository.".to_string())
}

fn check_claude_cli() -> Result<String> {
    use std::process::Command;

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message("Checking Claude CLI installation...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let home = env::var("HOME").unwrap_or_default();
    let claude_paths = vec![
        "claude".to_string(),
        format!("{}/.local/bin/claude", home),
        format!("{}/.claude/local/claude", home),
        "/usr/local/bin/claude".to_string(),
    ];

    let mut claude_check = None;
    let mut used_path = String::new();

    for path in &claude_paths {
        let result = Command::new(path).arg("--version").output();

        if result.is_ok() {
            claude_check = Some(result);
            used_path = path.clone();
            break;
        }
    }

    match claude_check {
        Some(Ok(output)) => {
            if output.status.success() {
                spinner.finish_with_message("✓ Claude CLI found");
                Ok(used_path)
            } else {
                spinner.finish_and_clear();
                let error_msg = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!(
                    "Claude Code encountered an error: {}\n\n\
                    Try reinstalling Claude Code from https://claude.ai/code",
                    error_msg.trim()
                )
            }
        }
        Some(Err(e)) => {
            spinner.finish_and_clear();
            anyhow::bail!("Failed to run Claude CLI at {}: {}", used_path, e)
        }
        None => {
            spinner.finish_and_clear();
            anyhow::bail!(
                "Claude Code is not installed or not in expected locations.\n\n\
                Checked locations:\n\
                - claude (in PATH)\n\
                - ~/.local/bin/claude\n\
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
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message("Analyzing git diff...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    
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

    spinner.finish_with_message("✓ Diff analyzed");
    Ok(diff_text)
}

fn get_commit_diff(repo: &Repository, commit_hash: &str) -> Result<String> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message(format!("Analyzing commit {}...", short_hash(commit_hash)));
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));
    
    let oid = Oid::from_str(commit_hash).context("Invalid commit hash")?;

    let commit = repo.find_commit(oid).context("Commit not found")?;

    let commit_tree = commit.tree()?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let mut diff_options = DiffOptions::new();
    let diff = repo.diff_tree_to_tree(
        parent_tree.as_ref(),
        Some(&commit_tree),
        Some(&mut diff_options),
    )?;

    let mut diff_text = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(content) = std::str::from_utf8(line.content()) {
            diff_text.push_str(content);
        }
        true
    })?;

    spinner.finish_with_message("✓ Commit diff analyzed");
    Ok(diff_text)
}

async fn generate_commit_message(diff: &str, long_format: bool, claude_path: &str) -> Result<String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use tokio::time::{timeout, Duration};
    
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    spinner.set_message("Generating commit message with Claude...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

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

    // If diff is very large, truncate it to avoid overwhelming the CLI.
    // Both the threshold and truncation use bytes for consistency — the concern
    // is data size, not character count. truncate_to_byte_limit ensures we land
    // on a valid UTF-8 char boundary.
    let truncated_prompt = if prompt.len() > MAX_PROMPT_BYTES {
        let truncated_diff = truncate_to_byte_limit(diff, TRUNCATED_DIFF_BYTES);
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
        stdin
            .write_all(truncated_prompt.as_bytes())
            .context("Failed to write prompt to Claude CLI stdin")?;
    }

    // Wait for output with timeout (increased for large commits)
    let output = tokio::task::spawn_blocking(move || child.wait_with_output());

    let output = timeout(Duration::from_secs(120), output)
        .await
        .context("Claude CLI timed out after 120 seconds. This may happen with large commits or complex diffs. Try using the --short flag for a more concise message.")?
        .context("Failed to join Claude CLI task")?
        .context("Failed to wait for Claude CLI output")?;

    if !output.status.success() {
        spinner.finish_and_clear();
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("not authenticated") || error.contains("login") {
            anyhow::bail!(
                "Claude Code is not authenticated.\n\n\
                Please run: claude login\n\
                Then authenticate with your Claude.ai account."
            );
        }
        spinner.finish_and_clear();
        anyhow::bail!("Claude CLI error: {}", error);
    }

    let mut message = String::from_utf8(output.stdout)?.trim().to_string();
    
    // Strip markdown code block formatting if present
    // Claude sometimes wraps responses in ```
    message = strip_markdown_code_block(&message);

    if message.is_empty() {
        spinner.finish_and_clear();
        anyhow::bail!("Claude CLI returned empty message");
    }

    // Check if the message is "Execution error" or other error patterns which indicate Claude CLI failed
    if message == "Execution error" 
        || message.starts_with("Error:") 
        || message.starts_with("error:") 
        || message.contains("failed")
        || message.contains("Failed") {
        // Log the stderr output for debugging
        let stderr_output = String::from_utf8_lossy(&output.stderr);
        eprintln!("Error: Claude CLI execution failed");
        eprintln!("Claude CLI output: {}", message);
        if !stderr_output.is_empty() {
            eprintln!("Claude CLI stderr: {}", stderr_output);
        }
        anyhow::bail!(
            "Claude CLI returned an error message: '{}'\n\n\
            This typically means:\n\
            - The Claude CLI encountered an internal error\n\
            - There may be an issue with your Claude.ai subscription or API limits\n\
            - The prompt may have been too large or malformed\n\n\
            Please try again or check your Claude Code installation with: claude --version",
            message
        );
    }
    
    // Additional safety check: ensure the message looks like a valid commit message
    // (not just an error or diagnostic output)
    if message.len() < 3 || !message.chars().any(|c| c.is_alphabetic()) {
        eprintln!("Warning: Generated message appears to be invalid: '{}'", message);
        anyhow::bail!(
            "Claude CLI returned an invalid commit message: '{}'\n\
            The message is too short or doesn't contain any letters.\n\
            Please try again.",
            message
        );
    }

    spinner.finish_with_message("✓ Commit message generated");
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
        Some("HEAD"),      // update_ref
        Some(&author),     // author (None keeps original)
        Some(&committer),  // committer (None keeps original)
        None,              // message_encoding (None for UTF-8)
        Some(new_message), // new message
        None,              // tree (None keeps original tree)
    )?;

    println!(
        "Commit successfully reworded! New commit: {}",
        amended_commit
    );

    Ok(())
}

fn reword_commit_with_rebase(
    repo: &Repository,
    commit_hash: &str,
    new_message: &str,
) -> Result<()> {
    use git2::RebaseOptions;

    let target_oid = Oid::from_str(commit_hash)?;
    let target_commit = repo.find_commit(target_oid)?;

    // Handle root commit specially - use amend instead of rebase
    if target_commit.parent_count() == 0 {
        // For root commits, we need to check if this is HEAD
        let head = repo.head()?.peel_to_commit()?;
        if head.id() == target_oid {
            // If it's HEAD, we can use the regular amend function
            amend_commit_with_git2(repo, new_message)?;
        } else {
            // If it's not HEAD but still a root commit, we need to handle it differently
            // Create a new root commit with the same tree but different message
            let author = target_commit.author();
            let committer = repo.signature()?;
            let tree = target_commit.tree()?;
            
            // Create the new root commit
            let _new_oid = repo.commit(
                None,             // don't update any refs yet
                &author,          // use original author
                &committer,       // use current committer
                new_message,      // new message
                &tree,            // same tree
                &[],              // no parents (root commit)
            )?;
            
            // Now we need to update the branch to point to the new commit
            // This is complex because we need to rebase all subsequent commits
            anyhow::bail!(
                "Rewording a root commit that is not HEAD requires rebasing the entire history. \
                Please checkout the root commit first with 'git checkout {}' then use --reword",
                short_hash(commit_hash)
            );
        }

        println!("Root commit successfully reworded!");
        return Ok(());
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
        Some(&branch_annotated), // branch (current branch)
        Some(&parent_annotated), // upstream (rebase onto parent of target)
        None,                    // onto (use upstream)
        Some(&mut rebase_options),
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
    println!(
        "\nWARNING: All commit hashes after {} have changed.",
        short_hash(commit_hash)
    );
    println!("If you've already pushed, you'll need to force push.");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Check if Claude CLI is available and get its path
    let claude_path = check_claude_cli()?;

    let repo = find_git_repository(args.path.as_deref())?;

    if args.reword {
        // Handle reword mode for the most recent commit
        let head = repo.head()?.peel_to_commit()?;
        let head_oid = head.id();
        let head_hash = head_oid.to_string();

        println!(
            "\nRewording the most recent commit: {}",
            short_hash(&head_hash)
        );
        println!("Original message: {}", head.message().unwrap_or(""));

        // Get the diff of the HEAD commit
        let commit_diff = get_commit_diff(&repo, &head_hash)?;

        let new_message = generate_commit_message(&commit_diff, !args.short, &claude_path).await?;

        println!("\nGenerated commit message:");
        println!("{}", new_message);

        if !args.dry_run {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
            );
            spinner.set_message("Amending commit...");
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            // Use git2 to amend the commit
            amend_commit_with_git2(&repo, &new_message)?;
            spinner.finish_with_message("✓ Commit amended successfully");

            println!(
                "\nWARNING: The commit hash has changed. If you've already pushed this commit,"
            );
            println!("you'll need to force push with: git push --force-with-lease");
        }
    } else if let Some(commit_hash) = args.fixup {
        // Handle fixup mode
        println!(
            "\nRewording commit {}...",
            short_hash(&commit_hash)
        );
        println!("This will:");
        println!("- Generate a new commit message using Claude");
        println!("- Update the commit message directly");
        println!("- Change all commit hashes after the target commit\n");

        // Get the diff of the commit to reword
        let commit_diff = get_commit_diff(&repo, &commit_hash)?;

        let new_message = generate_commit_message(&commit_diff, !args.short, &claude_path).await?;

        println!("\nGenerated commit message:");
        println!("{}", new_message);

        if !args.dry_run {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
            );
            spinner.set_message("Applying new commit message...");
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

            // Use git2 rebase to reword the commit
            reword_commit_with_rebase(&repo, &commit_hash, &new_message)?;
            spinner.finish_with_message("✓ Commit message updated");
        }
    } else {
        // Normal commit mode
        if args.all {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
            );
            spinner.set_message("Staging all changes...");
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));
            
            let mut index = repo.index()?;
            index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
            index.write()?;
            spinner.finish_with_message("✓ All changes staged");
        }

        let staged_diff = get_diff(&repo, true)?;
        if staged_diff.is_empty() {
            anyhow::bail!("No staged changes found. Use -a to stage all changes.");
        }

        let commit_message = generate_commit_message(&staged_diff, !args.short, &claude_path).await?;

        println!("\nGenerated commit message:");
        println!("{}", commit_message);

        if !args.dry_run {
            let spinner = ProgressBar::new_spinner();
            spinner.set_style(
                ProgressStyle::default_spinner()
                    .template("{spinner:.cyan} {msg}")
                    .unwrap()
                    .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
            );
            spinner.set_message("Creating commit...");
            spinner.enable_steady_tick(std::time::Duration::from_millis(100));

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

            spinner.finish_with_message("✓ Commit created successfully!");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- truncate_to_byte_limit ---

    #[test]
    fn truncate_ascii_within_limit() {
        assert_eq!(truncate_to_byte_limit("hello", 10), "hello");
    }

    #[test]
    fn truncate_ascii_at_exact_limit() {
        assert_eq!(truncate_to_byte_limit("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii_over_limit() {
        assert_eq!(truncate_to_byte_limit("hello world", 5), "hello");
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate_to_byte_limit("", 10), "");
    }

    #[test]
    fn truncate_japanese_chars() {
        // Each Japanese char is 3 bytes in UTF-8.
        // "日本語" = 9 bytes. Limit 7 should produce "日本" (6 bytes).
        assert_eq!(truncate_to_byte_limit("日本語", 7), "日本");
    }

    #[test]
    fn truncate_emoji() {
        // Each emoji is 4 bytes in UTF-8.
        // "🎉🎊🎁" = 12 bytes. Limit 5 should produce "🎉" (4 bytes).
        assert_eq!(truncate_to_byte_limit("🎉🎊🎁", 5), "🎉");
    }

    #[test]
    fn truncate_mixed_ascii_and_multibyte() {
        // "café" = 5 bytes (c=1, a=1, f=1, é=2). Limit 4 should produce "caf" (3 bytes).
        assert_eq!(truncate_to_byte_limit("café", 4), "caf");
    }

    #[test]
    fn truncate_zero_limit() {
        assert_eq!(truncate_to_byte_limit("hello", 0), "");
    }

    #[test]
    fn truncate_limit_one_with_multibyte() {
        // "é" is 2 bytes; limit 1 can't fit it, so result is empty.
        assert_eq!(truncate_to_byte_limit("é", 1), "");
    }

    // --- short_hash ---

    #[test]
    fn short_hash_full_sha() {
        assert_eq!(short_hash("55bad6c1234567890abcdef"), "55bad6c1");
    }

    #[test]
    fn short_hash_already_short() {
        assert_eq!(short_hash("abc"), "abc");
    }

    #[test]
    fn short_hash_exactly_8() {
        assert_eq!(short_hash("12345678"), "12345678");
    }

    #[test]
    fn short_hash_empty() {
        assert_eq!(short_hash(""), "");
    }

    // --- strip_markdown_code_block ---

    #[test]
    fn strip_no_fences() {
        assert_eq!(strip_markdown_code_block("fix: update deps"), "fix: update deps");
    }

    #[test]
    fn strip_opening_and_closing_fences() {
        let input = "```\nfix: update deps\n```";
        assert_eq!(strip_markdown_code_block(input), "fix: update deps");
    }

    #[test]
    fn strip_fences_with_language_identifier() {
        let input = "```text\nfix: update deps\n\nSome body.\n```";
        assert_eq!(strip_markdown_code_block(input), "fix: update deps\n\nSome body.");
    }

    #[test]
    fn strip_only_opening_fence() {
        let input = "```\nfix: update deps";
        assert_eq!(strip_markdown_code_block(input), "fix: update deps");
    }

    #[test]
    fn strip_only_closing_fence() {
        let input = "fix: update deps\n```";
        assert_eq!(strip_markdown_code_block(input), "fix: update deps");
    }

    #[test]
    fn strip_fences_with_multibyte_content() {
        let input = "```\nfix: 日本語の修正\n```";
        assert_eq!(strip_markdown_code_block(input), "fix: 日本語の修正");
    }

    #[test]
    fn strip_empty_code_block() {
        let input = "```\n```";
        assert_eq!(strip_markdown_code_block(input), "");
    }

    #[test]
    fn strip_backticks_only_no_newline() {
        // "``````" — opening ``` stripped, then closing ``` stripped
        let input = "``````";
        assert_eq!(strip_markdown_code_block(input), "");
    }
}
