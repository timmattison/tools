use anyhow::Result;
use chrono::Local;
use clap::Parser;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

mod cli;
mod date;
mod git;
mod ollama;
mod results;
mod stats;
mod ui;

use cli::Args;
use git::{get_git_user_email, ProgressCallback, ScanOptions, ScanProgress, SearchResult};
use results::{format_results, write_output_file};
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

    // Create scan options
    let scan_options = ScanOptions {
        search_all_branches: args.all,
        filter_by_user: args.filter_user,
        find_nested: args.find_nested,
        ignore_failures: args.ignore_failures,
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
            let options = scan_options.clone();

            thread::spawn(move || {
                // Check for cancellation before starting
                if scanning_cancelled.load(Ordering::Relaxed) {
                    return;
                }

                let progress = ScanProgress {
                    dirs_checked: &dirs_checked,
                    repos_found: &repos_found,
                    progress_callback: Some(&progress_callback),
                };

                // Perform the actual directory scan
                if let Err(e) = git::scan_path(&path, &result, &user_email, &options, &progress) {
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
            if io::stdout().is_terminal() {
                // Run interactive UI
                if let Err(e) = progress.run_interactive() {
                    eprintln!("UI error: {}", e);
                }
                // Signal cancellation when UI exits
                scanning_cancelled.store(true, Ordering::Relaxed);
            } else {
                // Simple progress output
                while !progress.is_cancelled()
                    && !scanning_cancelled.load(Ordering::Relaxed)
                    && !progress.is_all_complete()
                {
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

    // Format results (and run Ollama, if enabled) in a background task
    // while the UI is still running. `format_results` is a pure function:
    // it builds the output string but does NOT write any files.
    let format_handle = {
        let result = Arc::clone(&search_result);
        let args = args.clone();
        let progress_clone = Arc::clone(&progress);
        let cancellation_token = progress.cancellation_token();

        tokio::task::spawn_blocking(move || {
            let result_data = {
                let guard = result.lock().unwrap();
                guard.clone()
            };
            tokio::runtime::Handle::current().block_on(async move {
                format_results(
                    &result_data,
                    &args,
                    use_ollama,
                    Some(progress_clone),
                    cancellation_token,
                )
                .await
            })
        })
    };

    // Check if user cancelled
    let user_cancelled = progress.is_cancelled();

    // Wait for formatting to complete (only if not cancelled) and capture the buffer
    let formatted_output: Option<String> = if user_cancelled {
        None
    } else {
        let buffer = match format_handle.await {
            Ok(Ok(buffer)) => Some(buffer),
            Ok(Err(e)) => {
                eprintln!("Error formatting results: {}", e);
                None
            }
            Err(e) => {
                eprintln!("Error joining format task: {}", e);
                None
            }
        };
        if use_ollama {
            progress.set_ollama_complete();
        }
        buffer
    };

    // Wait for UI to finish
    let _ = ui_handle.join();

    // After the TUI exits, print the captured output to stdout exactly
    // once and write it to a file exactly once. Previously this block
    // re-ran `format_results` (duplicating work and creating a second
    // timestamped output file one second later).
    if let Some(buffer) = formatted_output {
        print!("{}", buffer);
        io::stdout().flush().ok();

        match write_output_file(&args, &buffer) {
            Ok(Some(path)) => println!("📝 Results written to {}", path.display()),
            Ok(None) => {}
            Err(e) => println!("⚠️  Error writing to output file: {}", e),
        }
    }

    Ok(())
}
