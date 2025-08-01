use anyhow::{Context, Result};
use clap::Parser;
use git2::{DiffOptions, Repository};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(author, version, about = "Automatically generate git commit messages using Claude")]
struct Args {
    #[arg(short, long, help = "Path to repository (defaults to current directory)")]
    path: Option<String>,
    
    #[arg(short, long, help = "Don't actually create the commit, just show the message")]
    dry_run: bool,
    
    #[arg(short, long, help = "Stage all modified files before committing")]
    all: bool,
}

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<Content>,
}

#[derive(Deserialize)]
struct Content {
    text: String,
}

fn get_api_key() -> Result<String> {
    let output = Command::new("security")
        .args(&["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .context("Failed to retrieve Claude credentials")?;
    
    if !output.status.success() {
        anyhow::bail!("Failed to retrieve API key from keychain");
    }
    
    let key = String::from_utf8(output.stdout)?
        .trim()
        .to_string();
    
    Ok(key)
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

async fn generate_commit_message(diff: &str, api_key: &str) -> Result<String> {
    let client = reqwest::Client::new();
    
    let prompt = format!(
        "Based on the following git diff, generate a clear and concise commit message. \
        Follow conventional commit format (type: description). \
        The message should explain what was changed and why, not just describe the diff. \
        Keep it under 72 characters for the subject line.\n\n{}",
        diff
    );
    
    let request = ClaudeRequest {
        model: "claude-3-5-sonnet-20241022".to_string(),
        max_tokens: 300,
        messages: vec![Message {
            role: "user".to_string(),
            content: prompt,
        }],
    };
    
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Claude API")?;
    
    if !response.status().is_success() {
        let error_text = response.text().await?;
        anyhow::bail!("Claude API error: {}", error_text);
    }
    
    let claude_response: ClaudeResponse = response.json().await?;
    let message = claude_response
        .content
        .first()
        .ok_or_else(|| anyhow::anyhow!("No response from Claude"))?
        .text
        .trim()
        .to_string();
    
    Ok(message)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    let repo_path = args.path.as_deref().unwrap_or(".");
    let repo = Repository::open(repo_path)
        .context("Failed to open git repository")?;
    
    if args.all {
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
    }
    
    let staged_diff = get_diff(&repo, true)?;
    if staged_diff.is_empty() {
        anyhow::bail!("No staged changes found. Use -a to stage all changes.");
    }
    
    let api_key = get_api_key()?;
    
    println!("Generating commit message...");
    let commit_message = generate_commit_message(&staged_diff, &api_key).await?;
    
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
    
    Ok(())
}