use anyhow::{anyhow, Result};
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::git::CommitInfo;

/// Status callback function type
pub type StatusCallback = Box<dyn Fn(&str) + Send + Sync>;

/// Model context sizes for different Ollama models
const MODEL_CONTEXT_SIZES: &[(&str, usize)] = &[
    ("llama2:7b", 4096),
    ("llama2:13b", 4096),
    ("mistral:7b", 8192),
    ("qwen3:30b-a3b", 32768),
];

/// Default context size if model not found
const DEFAULT_CONTEXT_SIZE: usize = 8192;

/// Ollama API request structure
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

/// Ollama API response chunk structure
#[derive(Deserialize)]
struct OllamaResponseChunk {
    model: Option<String>,
    created_at: Option<String>,
    response: String,
    done: bool,
}

/// Ollama API non-streaming response structure
#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Ollama client for generating summaries
pub struct OllamaClient {
    client: Client,
    base_url: String,
}

impl OllamaClient {
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Generate a summary of commits for a repository
    pub async fn generate_summary(
        &self,
        repo_path: &Path,
        commits: &[CommitInfo],
        model: &str,
        keep_thinking: bool,
        status_callback: Option<StatusCallback>,
    ) -> Result<String> {
        if let Some(callback) = &status_callback {
            callback(&format!("Analyzing repository {}", repo_path.file_name().unwrap_or_default().to_string_lossy()));
        }

        let repo_context = format!("Repository: {}", repo_path.file_name().unwrap_or_default().to_string_lossy());

        // Try to read README file for additional context
        let readme_content = self.read_readme(repo_path).await;

        // Get detailed commit information
        let detailed_commits = self.format_commits(commits, repo_path, &status_callback).await?;

        // Format the prompt for Ollama
        if let Some(callback) = &status_callback {
            callback("Preparing prompt for Ollama summarization");
        }

        let prompt = format!(
            r#"Please summarize the work done in this repository based on these recent commits.

{}
{}
Recent commits:
{}

Detailed commit information:
{}

Please provide a concise summary of what was worked on in this repository. Focus on:
1. What features or changes were implemented
2. Any bug fixes or improvements
3. The overall purpose of the changes
4. Technical details that would be relevant to a developer

Use the commit information to understand the changes in depth."#,
            repo_context,
            readme_content,
            commits.iter().map(|c| c.message.as_str()).collect::<Vec<_>>().join("\n"),
            detailed_commits
        );

        if let Some(callback) = &status_callback {
            let prompt_size = prompt.len() / 1024;
            callback(&format!("Sending {} KB of data to Ollama for processing", prompt_size));
        }

        self.call_ollama(model, &prompt, true, keep_thinking, status_callback).await
    }

    /// Generate a meta-summary across multiple repositories
    pub async fn generate_meta_summary(
        &self,
        summaries: &[String],
        model: &str,
        duration: chrono::Duration,
        keep_thinking: bool,
        status_callback: Option<StatusCallback>,
    ) -> Result<String> {
        if let Some(callback) = &status_callback {
            callback("Generating meta-summary across all repositories");
        }

        let prompt = format!(
            r#"Please provide a comprehensive overview of all work done across multiple repositories over the past {}.

Here are summaries from each repository:

{}

Focus on the big picture rather than repeating details from individual repositories."#,
            format_duration(duration),
            summaries.join("\n\n---\n\n")
        );

        if let Some(callback) = &status_callback {
            let prompt_size = prompt.len() / 1024;
            callback(&format!("Sending {} KB of data to Ollama for meta-summary processing", prompt_size));
        }

        self.call_ollama(model, &prompt, false, keep_thinking, status_callback).await
    }

    /// Try to read README file from repository
    async fn read_readme(&self, repo_path: &Path) -> String {
        let readme_paths = [
            repo_path.join("README.md"),
            repo_path.join("README"),
            repo_path.join("readme.md"),
            repo_path.join("Readme.md"),
        ];

        for readme_path in &readme_paths {
            if let Ok(content) = tokio::fs::read_to_string(readme_path).await {
                return format!("README:\n```\n{}\n```\n\n", content);
            }
        }

        String::new()
    }

    /// Format commits for the prompt
    async fn format_commits(
        &self,
        commits: &[CommitInfo],
        _repo_path: &Path,
        status_callback: &Option<StatusCallback>,
    ) -> Result<String> {
        let mut detailed_commits = String::new();
        let total_commits = commits.len();

        for (i, commit) in commits.iter().enumerate() {
            if let Some(callback) = status_callback {
                if i % 5 == 0 {
                    callback(&format!(
                        "Analyzing commit {} of {} ({}%)",
                        i + 1,
                        total_commits,
                        (i + 1) * 100 / total_commits
                    ));
                }
            }

            detailed_commits.push_str(&format!(
                "COMMIT: {}\nAUTHOR: {} <{}>\nDATE: {}\nMESSAGE:\n{}\n\n",
                commit.hash,
                commit.author_name,
                commit.author_email,
                commit.date.format("%Y-%m-%d %H:%M:%S"),
                commit.full_message
            ));

            detailed_commits.push_str("---\n");
        }

        Ok(detailed_commits)
    }

    /// Call the Ollama API
    async fn call_ollama(
        &self,
        model: &str,
        prompt: &str,
        stream: bool,
        keep_thinking: bool,
        status_callback: Option<StatusCallback>,
    ) -> Result<String> {
        if let Some(callback) = &status_callback {
            callback("Preparing to send request to Ollama");
            
            let context_size = MODEL_CONTEXT_SIZES
                .iter()
                .find(|(m, _)| *m == model)
                .map(|(_, size)| *size)
                .unwrap_or(DEFAULT_CONTEXT_SIZE);

            let estimated_tokens = estimate_tokens(prompt);
            callback(&format!("Estimated tokens: {} (model context size: {})", estimated_tokens, context_size));

            if estimated_tokens as f64 > context_size as f64 * 0.9 {
                callback("Prompt is too large, may exceed model context window");
            }
        }

        let request = OllamaRequest {
            model: model.to_string(),
            prompt: prompt.to_string(),
            stream,
        };

        let url = format!("{}/api/generate", self.base_url);
        let response = self.client
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(anyhow!("Ollama API request failed: {}", response.status()));
        }

        if stream {
            self.handle_streaming_response(response, keep_thinking, status_callback).await
        } else {
            self.handle_non_streaming_response(response, keep_thinking, status_callback).await
        }
    }

    /// Handle streaming response from Ollama
    async fn handle_streaming_response(
        &self,
        response: Response,
        keep_thinking: bool,
        status_callback: Option<StatusCallback>,
    ) -> Result<String> {
        let mut full_response = String::new();
        let mut response_length = 0;
        let mut last_update = std::time::Instant::now();

        let text = response.text().await?;
        
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let chunk: OllamaResponseChunk = serde_json::from_str(line)
                .map_err(|e| anyhow!("Error parsing response chunk: {}", e))?;

            full_response.push_str(&chunk.response);
            response_length += chunk.response.len();

            // Update status every 500ms
            if let Some(callback) = &status_callback {
                if last_update.elapsed() > std::time::Duration::from_millis(500) {
                    callback(&format!("Receiving response from Ollama ({} characters so far)", response_length));
                    last_update = std::time::Instant::now();
                }
            }

            if chunk.done {
                if let Some(callback) = &status_callback {
                    callback(&format!("Response complete, received {} characters", response_length));
                }
                break;
            }
        }

        let response = if keep_thinking {
            full_response
        } else {
            remove_thinking_text(&full_response)
        };

        Ok(response)
    }

    /// Handle non-streaming response from Ollama
    async fn handle_non_streaming_response(
        &self,
        response: Response,
        keep_thinking: bool,
        status_callback: Option<StatusCallback>,
    ) -> Result<String> {
        if let Some(callback) = &status_callback {
            callback("Waiting for complete response from Ollama");
        }

        let response_text = response.text().await?;
        
        if let Some(callback) = &status_callback {
            callback(&format!("Received {} KB response from Ollama", response_text.len() / 1024));
        }

        let ollama_response: OllamaResponse = serde_json::from_str(&response_text)
            .map_err(|e| anyhow!("Error parsing JSON response: {}", e))?;

        let response = if keep_thinking {
            ollama_response.response
        } else {
            remove_thinking_text(&ollama_response.response)
        };

        if let Some(callback) = &status_callback {
            callback(&format!("Processing complete, final response is {} characters", response.len()));
        }

        Ok(response)
    }
}

/// Estimate the number of tokens in a text (rough approximation)
fn estimate_tokens(text: &str) -> usize {
    // Rough estimate: 1 token ≈ 4 characters for English text
    text.len() / 4
}

/// Remove text between <think> and </think> tags
fn remove_thinking_text(text: &str) -> String {
    let mut result = text.to_string();
    
    while let Some(start) = result.find("<think>") {
        if let Some(end_pos) = result[start..].find("</think>") {
            let end = start + end_pos + "</think>".len();
            result.replace_range(start..end, "");
        } else {
            break;
        }
    }
    
    result
}

/// Format a chrono::Duration for display
fn format_duration(duration: chrono::Duration) -> String {
    let days = duration.num_days();
    let hours = duration.num_hours() % 24;
    let minutes = duration.num_minutes() % 60;

    if days > 0 {
        format!("{} day{}", days, if days == 1 { "" } else { "s" })
    } else if hours > 0 {
        format!("{} hour{}", hours, if hours == 1 { "" } else { "s" })
    } else {
        format!("{} minute{}", minutes, if minutes == 1 { "" } else { "s" })
    }
}