use anyhow::{Context, Result, anyhow};
use clap::Parser;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use tiktoken_rs::CoreBPE;

/// Token counter - estimate token counts for files
#[derive(Parser)]
#[command(name = "tc")]
#[command(about = "Count tokens in files (like wc, but for tokens)", long_about = None)]
#[command(version)]
struct Cli {
    /// Files to count tokens in (use '-' for stdin)
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,

    /// Tokenizer model to use
    #[arg(long, default_value = "gpt-4", value_name = "MODEL")]
    model: String,

    /// Show token count for each file individually
    #[arg(long)]
    per_file: bool,
}

/// Supported tokenizer models
#[derive(Debug, Clone)]
enum TokenizerModel {
    Gpt35Turbo,
    Gpt4,
    Gpt4o,
    Claude,
}

impl TokenizerModel {
    /// Parse model name from string
    ///
    /// # Arguments
    /// * `name` - The model name string to parse
    ///
    /// # Returns
    /// * `Result<TokenizerModel>` - The parsed model or an error if unsupported
    fn from_string(name: &str) -> Result<Self> {
        match name.to_lowercase().as_str() {
            "gpt-3.5-turbo" | "gpt-3.5" | "gpt35" => Ok(TokenizerModel::Gpt35Turbo),
            "gpt-4" | "gpt4" => Ok(TokenizerModel::Gpt4),
            "gpt-4o" | "gpt4o" => Ok(TokenizerModel::Gpt4o),
            "claude" | "claude-3-5-sonnet" | "claude-3" => Ok(TokenizerModel::Claude),
            _ => Err(anyhow!("Unsupported model: {}. Supported models: gpt-3.5-turbo, gpt-4, gpt-4o, claude", name)),
        }
    }

    /// Get the tokenizer for this model
    ///
    /// # Returns
    /// * `Result<CoreBPE>` - The tokenizer or an error if it cannot be loaded
    fn get_tokenizer(&self) -> Result<CoreBPE> {
        match self {
            TokenizerModel::Gpt35Turbo => Ok(tiktoken_rs::cl100k_base()?),
            TokenizerModel::Gpt4 => Ok(tiktoken_rs::cl100k_base()?),
            TokenizerModel::Gpt4o => Ok(tiktoken_rs::o200k_base()?),
            TokenizerModel::Claude => Ok(tiktoken_rs::cl100k_base()?), // Claude uses a similar tokenizer
        }
    }
}

/// Format a number with thousands separators
///
/// # Arguments
/// * `n` - The number to format
///
/// # Returns
/// * `String` - The formatted number with commas as thousands separators
fn format_count(n: usize) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;

    for c in s.chars().rev() {
        if count > 0 && count % 3 == 0 {
            result.push(',');
        }
        result.push(c);
        count += 1;
    }

    result.chars().rev().collect()
}

/// Count tokens in text using the specified tokenizer
///
/// # Arguments
/// * `text` - The text to count tokens in
/// * `tokenizer` - The tokenizer to use for counting
///
/// # Returns
/// * `usize` - The number of tokens in the text
fn count_tokens(text: &str, tokenizer: &CoreBPE) -> usize {
    tokenizer.encode_with_special_tokens(text).len()
}

/// Read content from a file
///
/// # Arguments
/// * `path` - The path to the file to read
///
/// # Returns
/// * `Result<String>` - The file contents or an error
fn read_file(path: &PathBuf) -> Result<String> {
    fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))
}

/// Read content from stdin
///
/// # Returns
/// * `Result<String>` - The stdin contents or an error
fn read_stdin() -> Result<String> {
    let mut buffer = String::new();
    io::stdin()
        .read_to_string(&mut buffer)
        .context("Failed to read from stdin")?;
    Ok(buffer)
}

/// Information about a file and its token count
struct FileTokenCount {
    path: String,
    token_count: usize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Parse the model
    let model = TokenizerModel::from_string(&cli.model)?;

    // Get the tokenizer
    let tokenizer = model.get_tokenizer()
        .context("Failed to load tokenizer")?;

    let mut file_counts = Vec::new();
    let mut total_tokens = 0usize;

    // If no files specified, read from stdin
    if cli.files.is_empty() {
        let content = read_stdin()?;
        let token_count = count_tokens(&content, &tokenizer);
        println!("{} tokens", format_count(token_count));
        return Ok(());
    }

    // Process each file
    for path in &cli.files {
        let (content, display_path) = if path.to_str() == Some("-") {
            (read_stdin()?, "stdin".to_string())
        } else {
            (read_file(path)?, path.display().to_string())
        };

        let token_count = count_tokens(&content, &tokenizer);
        total_tokens += token_count;

        file_counts.push(FileTokenCount {
            path: display_path,
            token_count,
        });
    }

    // Output results
    if cli.files.len() == 1 {
        // Single file: just show the count and filename
        let file = &file_counts[0];
        println!("{} tokens  {}", format_count(file.token_count), file.path);
    } else if cli.per_file {
        // Multiple files with per-file flag: show each file and total
        for file in &file_counts {
            println!("{} tokens  {}", format_count(file.token_count), file.path);
        }
        println!("-------");
        println!("{} tokens  total", format_count(total_tokens));
    } else {
        // Multiple files without per-file flag: just show total
        println!("{} tokens  total", format_count(total_tokens));
    }

    Ok(())
}
