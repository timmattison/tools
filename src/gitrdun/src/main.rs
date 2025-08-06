use anyhow::Result;
use chrono::Local;
use clap::Parser;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod cli;
mod date;
mod git;
mod ollama;
mod stats;
mod ui;

use cli::Args;
use git::{get_git_user_email, ProgressCallback, SearchResult};
use ollama::OllamaClient;
use ui::ProgressDisplay;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();


    // Parse the start string
    let start_duration = date::parse_duration(&args.start)?;
    let threshold_time = Local::now() - start_duration;

    // Parse the end string if provided
    let end_time = if let Some(end_str) = &args.end {
        Some(date::parse_time_string(end_str)?)
    } else {
        None
    };

    // If meta-ollama is set, enable ollama as well
    let use_ollama = args.ollama || args.meta_ollama;

    // Determine search paths
    let paths = if let Some(root) = &args.root {
        vec![root.clone()]
    } else if !args.paths.is_empty() {
        args.paths.clone()
    } else {
        // Default case: check if we're in a git repository
        let current_dir = PathBuf::from(".");
        if let Some(repo_root) = git::get_repository_root(&current_dir) {
            vec![repo_root]
        } else {
            vec![current_dir]
        }
    };

    // Get git user email
    let user_email = get_git_user_email(&stats::GitStats::new())?;

    // Create search result
    let search_result = Arc::new(Mutex::new(SearchResult::new(threshold_time, end_time)));

    // Create shared atomic counters for progress tracking
    let dirs_checked = Arc::new(AtomicUsize::new(0));
    let repos_found = Arc::new(AtomicUsize::new(0));
    let scanning_cancelled = Arc::new(AtomicBool::new(false));

    // Create progress display
    let progress = Arc::new(ProgressDisplay::new(threshold_time, end_time, use_ollama));

    // Create progress callback
    let progress_callback: Arc<ProgressCallback> = {
        let progress = Arc::clone(&progress);
        Arc::new(move |dirs: usize, repos: usize, current_path: &str| {
            progress.update_progress(dirs, repos, current_path.to_string());
        })
    };

    // Start the scanning process in background threads
    let scan_handles: Vec<_> = paths
        .iter()
        .map(|path| {
            let path = path.clone();
            let result = Arc::clone(&search_result);
            let dirs_checked = Arc::clone(&dirs_checked);
            let repos_found = Arc::clone(&repos_found);
            let scanning_cancelled = Arc::clone(&scanning_cancelled);
            let user_email = user_email.clone();
            let progress_callback = Arc::clone(&progress_callback);
            
            let search_all_branches = args.all;
            let filter_by_user = args.filter_user;
            let find_nested = args.find_nested;
            let ignore_failures = args.ignore_failures;

            thread::spawn(move || {
                // Check for cancellation before starting
                if scanning_cancelled.load(Ordering::Relaxed) {
                    return;
                }
                
                // Perform the actual directory scan
                if let Err(e) = git::scan_path(
                    &path,
                    &result,
                    &user_email,
                    search_all_branches,
                    filter_by_user,
                    find_nested,
                    ignore_failures,
                    &dirs_checked,
                    &repos_found,
                    Some(&progress_callback),
                ) {
                    eprintln!("Error scanning {}: {}", path.display(), e);
                }
            })
        })
        .collect();

    // Run the progress UI
    let ui_handle = {
        let progress = Arc::clone(&progress);
        let scanning_cancelled = Arc::clone(&scanning_cancelled);
        thread::spawn(move || {
            if atty::is(atty::Stream::Stdout) {
                // Run interactive UI
                if let Err(e) = progress.run_interactive() {
                    eprintln!("UI error: {}", e);
                }
                // Signal cancellation when UI exits
                scanning_cancelled.store(true, Ordering::Relaxed);
            } else {
                // Simple progress output
                while !progress.is_cancelled() && !scanning_cancelled.load(Ordering::Relaxed) && !progress.is_all_complete() {
                    progress.print_simple_progress();
                    thread::sleep(Duration::from_millis(100));
                }
                // Print final status
                if progress.is_all_complete() {
                    progress.print_simple_progress();
                }
            }
        })
    };

    // Wait for scanning to complete or cancellation
    for handle in scan_handles {
        let _ = handle.join();
    }

    // Signal that scanning is complete
    progress.set_scan_complete();
    
    // Process and display results in a separate task while UI is still running
    let display_handle = {
        let result = Arc::clone(&search_result);
        let args = args.clone();
        let progress_clone = Arc::clone(&progress);
        let cancellation_token = progress.cancellation_token();
        
        // We'll use spawn_blocking to avoid the nested runtime issue
        tokio::task::spawn_blocking(move || {
            // Block on the async operation
            tokio::runtime::Handle::current().block_on(async move {
                let result_guard = result.lock().unwrap();
                if let Err(e) = display_results(&*result_guard, &args, use_ollama, Some(progress_clone), cancellation_token).await {
                    eprintln!("Error displaying results: {}", e);
                }
            })
        })
    };
    
    // Check if user cancelled
    let user_cancelled = progress.is_cancelled();
    
    if !user_cancelled {
        // Wait for display task to complete only if not cancelled
        let _ = display_handle.await;
        
        // Signal that Ollama processing is complete (if it was running)
        if use_ollama {
            progress.set_ollama_complete();
        }
    }
    
    // Wait for UI to finish
    let _ = ui_handle.join();
    
    // After TUI exits, print the results to stdout (if not cancelled)
    if !user_cancelled {
        let result = search_result.lock().unwrap();
        // Use a new cancellation token that won't be cancelled for final output
        display_results(&*result, &args, use_ollama, None, CancellationToken::new()).await?;
    }

    Ok(())
}

/// Generate an automatic filename for saving results
fn generate_auto_filename(args: &Args) -> PathBuf {
    let now = Local::now();
    let timestamp = now.format("%Y-%m-%d-%H%M%S");
    let duration_str = args.start.replace(' ', "").replace(':', "");
    
    let filename = if args.end.is_some() {
        format!("gitrdun-results-{}-{}-to-end.txt", timestamp, duration_str)
    } else {
        format!("gitrdun-results-{}-{}.txt", timestamp, duration_str)
    };
    
    PathBuf::from(filename)
}

async fn display_results(result: &SearchResult, args: &Args, use_ollama: bool, progress: Option<Arc<ProgressDisplay>>, cancellation_token: CancellationToken) -> Result<()> {
    // Create output buffer for file writing
    let mut output_buffer = String::new();

    // Helper function to write to buffer and optionally stdout
    let has_progress = progress.is_some();
    let mut write_output = |text: &str| {
        output_buffer.push_str(text);
        // Only print to stdout if we're not in TUI mode
        if !has_progress {
            print!("{}", text);
            io::stdout().flush().unwrap();
        }
    };

    // Print inaccessible directories if any
    if !result.inaccessible_dirs.is_empty() && !args.ignore_failures {
        write_output("⚠️  The following directories could not be fully accessed:\n");
        for dir in &result.inaccessible_dirs {
            write_output(&format!("  {}\n", dir));
        }
        write_output("\n");
    }

    if result.found_commits {
        write_output("🔍 Found commits\n");
        write_output(&format!("📅 Start date: {}\n", result.threshold.format("%A, %B %d, %Y at %l:%M %p")));
        if let Some(end) = result.end_time {
            write_output(&format!("📅 End date: {}\n", end.format("%A, %B %d, %Y at %l:%M %p")));
        }
        write_output(&format!("📂 Search paths: {}\n", result.abs_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
        if args.all {
            write_output("🔀 Searching across all branches\n");
        }
        write_output("\n");

        // Calculate total commits
        let total_commits: usize = result.repositories.values().map(|commits| commits.len()).sum();

        write_output("📊 Summary:\n");
        write_output(&format!("   • Found {} commits across {} repositories\n\n", total_commits, result.repositories.len()));

        // Sort repository paths for consistent output
        let mut sorted_repo_paths: Vec<_> = result.repositories.keys().collect();
        sorted_repo_paths.sort();

        // For meta-ollama, collect all summaries
        let mut all_summaries = Vec::new();

        // Initialize Ollama client if needed
        let ollama_client = if use_ollama {
            Some(OllamaClient::new(args.ollama_url.clone()))
        } else {
            None
        };


        // Display results in sorted order
        for repo_path in sorted_repo_paths {
            let commits = &result.repositories[repo_path];
            write_output(&format!("📁 {} - {} commits\n", repo_path.display(), commits.len()));

            if let Some(client) = &ollama_client {
                // Check if cancelled before processing
                if cancellation_token.is_cancelled() {
                    write_output("\n⚠️  Processing cancelled by user\n");
                    break;
                }
                
                // Show commits if not summary-only
                if !args.summary_only {
                    for commit in commits {
                        write_output(&format!("      • {}\n", commit.message));
                    }
                }

                // Generate Ollama summary
                let repo_name = repo_path.file_name().unwrap_or_default().to_string_lossy();
                let branch_name = git::get_current_branch(repo_path);
                let full_repo_info = format!("{} ({})", repo_path.display(), branch_name);
                write_output(&format!("\n🤖 Generating summary for {} with Ollama ({})...\n", repo_name, args.ollama_model));

                // Update progress display with current repo
                if let Some(progress_ref) = &progress {
                    progress_ref.update_ollama_repo(full_repo_info.clone());
                    progress_ref.update_ollama_status(format!("Generating summary for {}", full_repo_info));
                }

                let status_callback: Box<dyn Fn(&str) + Send + Sync> = if let Some(progress_ref) = &progress {
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

                // Use tokio::select! to make the operation cancellable
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

        // Generate meta-summary if requested
        if args.meta_ollama && !all_summaries.is_empty() && !cancellation_token.is_cancelled() {
            if let Some(client) = &ollama_client {
                write_output(&format!("\n🔍 Generating meta-summary of all work with Ollama ({})...\n", args.ollama_model));

                // Update progress display for meta-summary
                if let Some(progress_ref) = &progress {
                    progress_ref.update_ollama_repo("Meta-Summary".to_string());
                    progress_ref.update_ollama_status("Generating meta-summary of all work".to_string());
                }

                let status_callback: Box<dyn Fn(&str) + Send + Sync> = if let Some(progress_ref) = &progress {
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
                
                // Use tokio::select! to make the meta-summary cancellable
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

        // Mark Ollama as complete
        if let Some(progress_ref) = &progress {
            if use_ollama {
                progress_ref.set_ollama_complete();
            }
        }
    } else {
        write_output("😴 No commits found\n");
        write_output(&format!("   • Start date: {}\n", result.threshold.format("%A, %B %d, %Y at %l:%M %p")));
        if let Some(end) = result.end_time {
            write_output(&format!("   • End date: {}\n", end.format("%A, %B %d, %Y at %l:%M %p")));
        }
        write_output(&format!("   • Search paths: {}\n", result.abs_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")));
    }

    // Show stats if requested
    if args.stats {
        write_output("\n🔍 Git Operation Stats:\n");
        
        if let Ok(get_git_dir_stats) = result.stats.get_git_dir.lock() {
            write_output(&format!("   • getGitDir: {} calls, avg {:?} per call\n",
                get_git_dir_stats.count(),
                get_git_dir_stats.average()));
        }
        
        if let Ok(get_log_stats) = result.stats.get_log.lock() {
            write_output(&format!("   • git log: {} calls, avg {:?} per call\n",
                get_log_stats.count(),
                get_log_stats.average()));
        }
        
        if let Ok(get_email_stats) = result.stats.get_email.lock() {
            write_output(&format!("   • git config: {} calls, avg {:?} per call\n",
                get_email_stats.count(),
                get_email_stats.average()));
        }
        
        write_output("\n");
    }

    // Write to file (automatic by default, unless --no-file is specified)
    if !args.no_file {
        let output_file = if let Some(custom_file) = &args.output {
            custom_file.clone()
        } else {
            generate_auto_filename(args)
        };
        
        match std::fs::write(&output_file, &output_buffer) {
            Ok(_) => println!("📝 Results written to {}", output_file.display()),
            Err(e) => println!("⚠️  Error writing to output file: {}", e),
        }
    }

    Ok(())
}

// Implement Default for Args to support the pattern used above
impl Default for Args {
    fn default() -> Self {
        Self {
            start: "24h".to_string(),
            end: None,
            ignore_failures: false,
            summary_only: false,
            find_nested: false,
            stats: false,
            all: false,
            ollama: false,
            meta_ollama: false,
            ollama_model: "gpt-oss".to_string(),
            ollama_url: "http://localhost:11434".to_string(),
            root: None,
            output: None,
            no_file: false,
            filter_user: true,
            keep_thinking: false,
            paths: Vec::new(),
        }
    }
}