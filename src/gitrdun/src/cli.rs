use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(
    name = "gitrdun",
    author = "Tim Mattison", 
    about = "Find and summarize recent git commits across multiple repositories",
    version,
    long_about = "gitrdun recursively searches for Git repositories and finds recent commits within a specified time range. It can optionally generate AI-powered summaries using Ollama.",
    after_help = "Examples:
  gitrdun                                    # Find commits from the last 24 hours (saves to auto file)
  gitrdun --start 7d                         # Find commits from the last 7 days (saves to auto file)
  gitrdun --start monday --end friday        # Find commits between Monday and Friday
  gitrdun --ollama --ollama-model llama2:7b  # Generate summaries with Ollama
  gitrdun --no-file                          # Display results only, don't save to file
  gitrdun --output custom.txt                # Save to custom filename instead of auto"
)]
pub struct Args {
    /// How far back to start looking for commits (e.g. 24h, 7d, 2w, 'monday', 'last month', 'february', etc.)
    #[arg(long, default_value = "24h")]
    pub start: String,

    /// When to stop looking for commits (e.g. '2023-12-31', 'yesterday', 'last month', etc.)
    #[arg(long)]
    pub end: Option<String>,

    /// Suppress output about directories that couldn't be accessed
    #[arg(long = "ignore-failures")]
    pub ignore_failures: bool,

    /// Only show repository names and commit counts
    #[arg(long = "summary-only")]
    pub summary_only: bool,

    /// Look for nested git repositories inside other git repositories
    #[arg(long = "find-nested")]
    pub find_nested: bool,

    /// Show git operation statistics
    #[arg(long)]
    pub stats: bool,

    /// Search all branches, not just the current branch
    #[arg(long)]
    pub all: bool,

    /// Use Ollama to generate summaries of work done in each repository
    #[arg(long)]
    pub ollama: bool,

    /// Generate a meta-summary across all repositories (implies --ollama)
    #[arg(long = "meta-ollama")]
    pub meta_ollama: bool,

    /// Ollama model to use for summaries
    #[arg(long = "ollama-model", default_value = "qwen3:30b-a3b")]
    pub ollama_model: String,

    /// URL for Ollama API
    #[arg(long = "ollama-url", default_value = "http://localhost:11434")]
    pub ollama_url: String,

    /// Root directory to start scanning from (overrides positional arguments)
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Custom file to write results to (default: auto-generated filename)
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Disable automatic file saving (results are saved by default)
    #[arg(long = "no-file")]
    pub no_file: bool,


    /// Only show commits authored by the current git user
    #[arg(long = "filter-user", default_value = "true")]
    pub filter_user: bool,

    /// Keep text between <think> and </think> tags in LLM output
    #[arg(long = "keep-thinking")]
    pub keep_thinking: bool,

    /// Search paths (if not using --root)
    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,
}