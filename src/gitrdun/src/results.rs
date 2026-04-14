use anyhow::Result;
use chrono::Local;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::cli::Args;
use crate::date;
use crate::git::{self, SearchResult};
use crate::ollama::OllamaClient;
use crate::ui::ProgressDisplay;

/// Format search results into a string buffer.
///
/// This is a pure function with respect to the file system: it returns
/// the formatted output as a `String` without creating or modifying any
/// files. The caller is responsible for writing the result to a file or
/// to stdout.
///
/// When `progress` is `Some`, Ollama status updates are routed through
/// the UI instead of being printed to stdout.
///
/// # Errors
///
/// Returns an error if duration parsing fails for the meta-summary path.
///
/// # Panics
///
/// Panics if flushing stdout fails (only when `progress` is `None`,
/// which means output is being streamed directly to stdout).
pub async fn format_results(
    result: &SearchResult,
    args: &Args,
    use_ollama: bool,
    progress: Option<Arc<ProgressDisplay>>,
    cancellation_token: CancellationToken,
) -> Result<String> {
    let mut output_buffer = String::new();
    let has_progress = progress.is_some();

    let mut write_output = |text: &str| {
        output_buffer.push_str(text);
        if !has_progress {
            print!("{}", text);
            io::stdout().flush().unwrap();
        }
    };

    if !result.inaccessible_dirs.is_empty() && !args.ignore_failures {
        write_output("⚠️  The following directories could not be fully accessed:\n");
        for dir in &result.inaccessible_dirs {
            write_output(&format!("  {}\n", dir));
        }
        write_output("\n");
    }

    if result.found_commits {
        write_output("🔍 Found commits\n");
        write_output(&format!(
            "📅 Start date: {}\n",
            result.threshold.format("%A, %B %d, %Y at %l:%M %p")
        ));
        if let Some(end) = result.end_time {
            write_output(&format!(
                "📅 End date: {}\n",
                end.format("%A, %B %d, %Y at %l:%M %p")
            ));
        }
        write_output(&format!(
            "📂 Search paths: {}\n",
            result
                .abs_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if args.all {
            write_output("🔀 Searching across all branches\n");
        }
        write_output("\n");

        let total_commits: usize = result
            .repositories
            .values()
            .map(|commits| commits.len())
            .sum();

        write_output("📊 Summary:\n");
        write_output(&format!(
            "   • Found {} commits across {} repositories\n\n",
            total_commits,
            result.repositories.len()
        ));

        let mut sorted_repo_paths: Vec<_> = result.repositories.keys().collect();
        sorted_repo_paths.sort();

        let mut all_summaries = Vec::new();

        let ollama_client = if use_ollama {
            Some(OllamaClient::new(args.ollama_url.clone()))
        } else {
            None
        };

        for repo_path in sorted_repo_paths {
            let commits = &result.repositories[repo_path];
            write_output(&format!(
                "📁 {} - {} commits\n",
                repo_path.display(),
                commits.len()
            ));

            if let Some(client) = &ollama_client {
                if cancellation_token.is_cancelled() {
                    write_output("\n⚠️  Processing cancelled by user\n");
                    break;
                }

                if !args.summary_only {
                    for commit in commits {
                        write_output(&format!("      • {}\n", commit.message));
                    }
                }

                let repo_name = repo_path.file_name().unwrap_or_default().to_string_lossy();
                let branch_name = git::get_current_branch(repo_path);
                let full_repo_info = format!("{} ({})", repo_path.display(), branch_name);
                write_output(&format!(
                    "\n🤖 Generating summary for {} with Ollama ({})...\n",
                    repo_name, args.ollama_model
                ));

                if let Some(progress_ref) = &progress {
                    progress_ref.update_ollama_repo(full_repo_info.clone());
                    progress_ref
                        .update_ollama_status(format!("Generating summary for {}", full_repo_info));
                }

                let status_callback: Box<dyn Fn(&str) + Send + Sync> =
                    if let Some(progress_ref) = &progress {
                        let progress_clone = Arc::clone(progress_ref);
                        Box::new(move |status: &str| {
                            progress_clone.update_ollama_progress(status.to_string());
                        })
                    } else {
                        Box::new(|status: &str| {
                            print!("\r\x1b[K   ⏳ {}", status);
                            io::stdout().flush().unwrap();
                        })
                    };

                tokio::select! {
                    result = client.generate_summary(
                        repo_path,
                        commits,
                        &args.ollama_model,
                        args.keep_thinking,
                        Some(status_callback),
                    ) => {
                        match result {
                            Ok(summary) => {
                                write_output("\n");
                                write_output(&format!("📝 Summary for {} ({}): \n{}\n\n", repo_name, args.ollama_model, summary));

                                if args.meta_ollama {
                                    all_summaries.push(format!("Repository: {}\n{}", repo_path.display(), summary));
                                }
                            }
                            Err(e) => {
                                write_output(&format!("\n⚠️  Error generating summary: {}\n", e));
                            }
                        }
                    }
                    _ = cancellation_token.cancelled() => {
                        write_output("\n⚠️  Summary generation cancelled\n");
                        break;
                    }
                }
            } else if !args.summary_only {
                for commit in commits {
                    write_output(&format!("      • {}\n", commit.message));
                }
                write_output("\n");
            }
        }

        if args.meta_ollama && !all_summaries.is_empty() && !cancellation_token.is_cancelled() {
            if let Some(client) = &ollama_client {
                write_output(&format!(
                    "\n🔍 Generating meta-summary of all work with Ollama ({})...\n",
                    args.ollama_model
                ));

                if let Some(progress_ref) = &progress {
                    progress_ref.update_ollama_repo("Meta-Summary".to_string());
                    progress_ref
                        .update_ollama_status("Generating meta-summary of all work".to_string());
                }

                let status_callback: Box<dyn Fn(&str) + Send + Sync> =
                    if let Some(progress_ref) = &progress {
                        let progress_clone = Arc::clone(progress_ref);
                        Box::new(move |status: &str| {
                            progress_clone.update_ollama_progress(status.to_string());
                        })
                    } else {
                        Box::new(|status: &str| {
                            print!("\r\x1b[K   ⏳ {}", status);
                            io::stdout().flush().unwrap();
                        })
                    };

                let start_duration = date::parse_duration(&args.start)?;

                tokio::select! {
                    result = client.generate_meta_summary(
                        &all_summaries,
                        &args.ollama_model,
                        start_duration,
                        args.keep_thinking,
                        Some(status_callback),
                    ) => {
                        match result {
                            Ok(meta_summary) => {
                                write_output("\n");
                                write_output(&format!("\n📊 Meta-Summary of All Work ({}):\n{}\n", args.ollama_model, meta_summary));
                            }
                            Err(e) => {
                                write_output(&format!("\n⚠️  Error generating meta-summary: {}\n", e));
                            }
                        }
                    }
                    _ = cancellation_token.cancelled() => {
                        write_output("\n⚠️  Meta-summary generation cancelled\n");
                    }
                }
            }
        }

        if let Some(progress_ref) = &progress {
            if use_ollama {
                progress_ref.set_ollama_complete();
            }
        }
    } else {
        write_output("😴 No commits found\n");
        write_output(&format!(
            "   • Start date: {}\n",
            result.threshold.format("%A, %B %d, %Y at %l:%M %p")
        ));
        if let Some(end) = result.end_time {
            write_output(&format!(
                "   • End date: {}\n",
                end.format("%A, %B %d, %Y at %l:%M %p")
            ));
        }
        write_output(&format!(
            "   • Search paths: {}\n",
            result
                .abs_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    if args.stats {
        write_output("\n🔍 Git Operation Stats:\n");

        if let Ok(get_git_dir_stats) = result.stats.get_git_dir.lock() {
            write_output(&format!(
                "   • getGitDir: {} calls, avg {:?} per call\n",
                get_git_dir_stats.count(),
                get_git_dir_stats.average()
            ));
        }

        if let Ok(get_log_stats) = result.stats.get_log.lock() {
            write_output(&format!(
                "   • git log: {} calls, avg {:?} per call\n",
                get_log_stats.count(),
                get_log_stats.average()
            ));
        }

        if let Ok(get_email_stats) = result.stats.get_email.lock() {
            write_output(&format!(
                "   • git config: {} calls, avg {:?} per call\n",
                get_email_stats.count(),
                get_email_stats.average()
            ));
        }

        write_output("\n");
    }

    Ok(output_buffer)
}

/// Generate an automatic filename for saving results.
///
/// The filename embeds the current local time (to the second) and the
/// `--start` duration so multiple runs produce distinct files.
pub fn generate_auto_filename(args: &Args) -> PathBuf {
    let now = Local::now();
    let timestamp = now.format("%Y-%m-%d-%H%M%S");
    let duration_str = args.start.replace([' ', ':'], "");

    let filename = if args.end.is_some() {
        format!("gitrdun-results-{}-{}-to-end.txt", timestamp, duration_str)
    } else {
        format!("gitrdun-results-{}-{}.txt", timestamp, duration_str)
    };

    PathBuf::from(filename)
}

/// Write the formatted output buffer to disk according to `args`.
///
/// Honours `--no-file` (skip writing entirely) and `--output`
/// (custom filename). Returns the path that was written, if any.
///
/// # Errors
///
/// Returns any I/O error from `std::fs::write`.
pub fn write_output_file(args: &Args, content: &str) -> std::io::Result<Option<PathBuf>> {
    if args.no_file {
        return Ok(None);
    }
    let output_file = if let Some(custom_file) = &args.output {
        custom_file.clone()
    } else {
        generate_auto_filename(args)
    };
    std::fs::write(&output_file, content)?;
    Ok(Some(output_file))
}
