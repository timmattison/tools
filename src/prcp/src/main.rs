// Clippy lints to catch common issues
#![warn(clippy::unwrap_used)] // Prefer explicit error handling
#![warn(clippy::expect_used)] // Prefer explicit error handling
#![warn(clippy::panic)] // Avoid panics in library code
#![deny(clippy::unimplemented)] // Don't leave unimplemented!() in code
#![warn(clippy::cast_possible_truncation)] // Catch potential data loss in casts
#![warn(clippy::cast_sign_loss)] // Catch sign loss in casts
#![warn(clippy::cast_precision_loss)] // Catch precision loss in float casts
#![warn(clippy::large_futures)] // Catch futures too large to box efficiently
#![warn(clippy::semicolon_if_nothing_returned)] // Ensure consistent semicolon usage
#![warn(clippy::unused_async)] // Catch async functions that don't await

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task;

/// Default buffer size for I/O operations (16MB)
const DEFAULT_BUFFER_SIZE: usize = 16 * 1024 * 1024;

/// Minimum allowed buffer size (4KB) - smaller values cause excessive syscall overhead
const MIN_BUFFER_SIZE: usize = 4 * 1024;

/// Maximum allowed buffer size (1GB) - larger values risk OOM
const MAX_BUFFER_SIZE: usize = 1024 * 1024 * 1024;

/// Channel depth for parallel copy operations.
/// Higher values allow more read-ahead but use more memory.
const CHANNEL_DEPTH: usize = 4;

/// Threshold for using parallel hashing (1MB).
/// Below this size, single-threaded hashing is faster due to reduced thread overhead.
const PARALLEL_HASH_THRESHOLD: usize = 1024 * 1024;

/// Shared format string for progress stats (bytes, percentage, speed, ETA).
const PROGRESS_STATS_FORMAT: &str = "{bytes}/{total_bytes} ({percent}%) ({bytes_per_sec}, {eta})";

/// Progress bar characters for smooth progress visualization.
const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏  ";

/// RAII guard for terminal raw mode state management.
/// Ensures raw mode is properly restored even if an error occurs.
struct RawModeGuard {
    was_enabled: bool,
    currently_raw: bool,
}

impl RawModeGuard {
    /// Create a new guard and enable raw mode if possible
    fn new() -> Self {
        let was_enabled = crossterm::terminal::enable_raw_mode().is_ok();
        Self {
            was_enabled,
            currently_raw: was_enabled,
        }
    }

    /// Temporarily disable raw mode for user input (returns true if was enabled)
    fn disable_temporarily(&mut self) -> bool {
        if self.currently_raw {
            let _ = crossterm::terminal::disable_raw_mode();
            self.currently_raw = false;
            true
        } else {
            false
        }
    }

    /// Re-enable raw mode if it was previously enabled
    fn restore(&mut self) {
        if self.was_enabled && !self.currently_raw {
            let _ = crossterm::terminal::enable_raw_mode();
            self.currently_raw = true;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.currently_raw {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

/// RAII guard for cleaning up partial destination files on error.
/// Ensures the destination file is properly closed and removed if the copy fails.
/// This handles all error paths consistently, including Ctrl+C cancellation,
/// I/O errors, and any other failures during the copy operation.
struct PartialFileGuard<'a> {
    destination: &'a Path,
    file: Option<File>,
    defused: bool,
}

impl<'a> PartialFileGuard<'a> {
    /// Create a new guard that will clean up the destination file on drop unless defused.
    fn new(destination: &'a Path, file: File) -> Self {
        Self {
            destination,
            file: Some(file),
            defused: false,
        }
    }

    /// Get a mutable reference to the underlying file for I/O operations.
    ///
    /// # Panics
    /// Panics if called after `defuse()` - this is a programming error.
    #[allow(clippy::expect_used)] // Invariant: file is always Some until defuse()
    fn file_mut(&mut self) -> &mut File {
        self.file
            .as_mut()
            .expect("PartialFileGuard: file already consumed")
    }

    /// Defuse the guard and return the file for final operations.
    /// After calling this, the guard will NOT clean up the destination file on drop.
    /// Returns the file so the caller can perform final flush/sync operations.
    ///
    /// # Panics
    /// Panics if called more than once - this is a programming error.
    #[allow(clippy::expect_used)] // Invariant: file is always Some until defuse()
    fn defuse(mut self) -> File {
        self.defused = true;
        self.file
            .take()
            .expect("PartialFileGuard: file already consumed")
    }
}

impl Drop for PartialFileGuard<'_> {
    fn drop(&mut self) {
        if !self.defused {
            // Close the file handle first (important for Windows compatibility)
            drop(self.file.take());
            // Now remove the partial file - ignore errors since we're already in cleanup
            let _ = fs::remove_file(self.destination);
        }
    }
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Progress copy - copy files with progress bar", long_about = None)]
struct Args {
    /// Source file(s) and destination (last argument is destination)
    #[arg(num_args = 0..)]
    paths: Vec<PathBuf>,

    /// Add shell integration (prmv function) to your shell config
    #[arg(long)]
    shell_setup: bool,

    /// Remove source files after successful copy (verified by Blake3 hash)
    #[arg(long, short = 'r')]
    rm: bool,

    /// Skip Blake3 verification after copy (not allowed with --rm).
    /// Since Blake3 is extremely fast, verification typically adds minimal overhead.
    #[arg(long)]
    no_verify: bool,

    /// Continue copying remaining files if one fails
    #[arg(long)]
    continue_on_error: bool,

    /// Skip all confirmation prompts (assume yes)
    #[arg(long, short = 'y')]
    yes: bool,

    /// Buffer size for I/O operations in bytes (default: 16MB).
    /// Larger buffers can improve throughput on fast storage.
    #[arg(long, default_value_t = DEFAULT_BUFFER_SIZE, value_parser = parse_buffer_size)]
    buffer_size: usize,
}

/// Parse buffer size from string, supporting suffixes like K, M, G.
/// Validates that the size is within acceptable bounds (4KB - 1GB).
fn parse_buffer_size(s: &str) -> Result<usize, String> {
    let s = s.trim().to_uppercase();
    let (num_str, multiplier) = if s.ends_with("G") || s.ends_with("GB") {
        (s.trim_end_matches("GB").trim_end_matches('G'), 1024 * 1024 * 1024)
    } else if s.ends_with("M") || s.ends_with("MB") {
        (s.trim_end_matches("MB").trim_end_matches('M'), 1024 * 1024)
    } else if s.ends_with("K") || s.ends_with("KB") {
        (s.trim_end_matches("KB").trim_end_matches('K'), 1024)
    } else {
        (s.as_str(), 1)
    };

    let size = num_str
        .trim()
        .parse::<usize>()
        .map(|n| n.saturating_mul(multiplier))
        .map_err(|e| format!("Invalid buffer size '{}': {}", s, e))?;

    if size < MIN_BUFFER_SIZE {
        return Err(format!(
            "Buffer size {} is too small (minimum: 4KB)",
            format_size(size)
        ));
    }

    if size > MAX_BUFFER_SIZE {
        return Err(format!(
            "Buffer size {} is too large (maximum: 1GB)",
            format_size(size)
        ));
    }

    Ok(size)
}

/// Format a byte size for human-readable error messages
fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{}GB", bytes / (1024 * 1024 * 1024))
    } else if bytes >= 1024 * 1024 {
        format!("{}MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{}B", bytes)
    }
}

/// The shell integration code to add to shell config files.
const SHELL_INTEGRATION: &str = r#"
# prcp - Progress Copy shell integration
# Added by: prcp --shell-setup
function prmv() {
    prcp --rm "$@"
}
"#;

/// Marker to detect if shell integration is already installed.
const SHELL_INTEGRATION_MARKER: &str = "prcp - Progress Copy shell integration";

/// Sets up shell integration by adding the prmv function to the user's shell config.
fn setup_shell_integration() -> Result<()> {
    // Get home directory
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    // Detect shell from SHELL environment variable
    let shell = std::env::var("SHELL").unwrap_or_default();
    let shell_name = Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Determine which config file to use
    let config_file = match shell_name {
        "zsh" => home.join(".zshrc"),
        "bash" => {
            // Prefer .bashrc, but use .bash_profile on macOS if .bashrc doesn't exist
            let bashrc = home.join(".bashrc");
            let bash_profile = home.join(".bash_profile");
            if bashrc.exists() {
                bashrc
            } else if bash_profile.exists() {
                bash_profile
            } else {
                bashrc // Create .bashrc if neither exists
            }
        }
        _ => {
            anyhow::bail!(
                "Unsupported shell: {}. Please manually add the shell integration to your config.\n\
                 Add this to your shell config:\n{}",
                shell_name,
                SHELL_INTEGRATION
            );
        }
    };

    // Check if already installed
    if config_file.exists() {
        let contents = fs::read_to_string(&config_file)
            .with_context(|| format!("Could not read {}", config_file.display()))?;

        if contents.contains(SHELL_INTEGRATION_MARKER) {
            println!(
                "{} Shell integration already installed in {}",
                "✓".green(),
                config_file.display()
            );
            return Ok(());
        }
    }

    // Append shell integration to config file
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&config_file)
        .with_context(|| format!("Could not open {}", config_file.display()))?;

    file.write_all(SHELL_INTEGRATION.as_bytes())
        .with_context(|| format!("Could not write to {}", config_file.display()))?;

    println!(
        "{} Shell integration added to {}",
        "✓".green(),
        config_file.display()
    );
    println!();
    println!("To activate, run:");
    println!("  {} {}", "source".cyan(), config_file.display());
    println!();
    println!("Or open a new terminal window.");
    println!();
    println!("Available commands:");
    println!("  {} - Copy files with progress, removing sources after verification", "prmv".yellow());

    Ok(())
}

/// Resolve source patterns into a list of files
///
/// Handles both literal file paths and glob patterns (*, ?, []).
/// Returns an error if a glob pattern matches no files or a literal path doesn't exist.
/// Glob iteration errors (e.g., permission denied) are collected and reported.
fn resolve_sources(patterns: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut glob_errors: Vec<String> = Vec::new();

    for pattern in patterns {
        let pattern_str = pattern.to_string_lossy();

        // Check if pattern contains glob characters
        if pattern_str.contains('*') || pattern_str.contains('?') || pattern_str.contains('[') {
            let glob_iter = glob::glob(&pattern_str)
                .with_context(|| format!("Invalid glob pattern '{}'", pattern_str))?;

            let mut matches = Vec::new();
            for entry in glob_iter {
                match entry {
                    Ok(path) => {
                        if path.is_file() {
                            matches.push(path);
                        }
                    }
                    Err(e) => {
                        // Collect glob errors (e.g., permission denied) for reporting
                        glob_errors.push(format!("{}: {}", e.path().display(), e.error()));
                    }
                }
            }

            if matches.is_empty() {
                if glob_errors.is_empty() {
                    anyhow::bail!("No files match pattern '{}'", pattern_str);
                } else {
                    anyhow::bail!(
                        "No files match pattern '{}'. Errors encountered:\n  {}",
                        pattern_str,
                        glob_errors.join("\n  ")
                    );
                }
            }
            files.extend(matches);
        } else {
            // Literal path - validate it exists and is a file
            if !pattern.exists() {
                anyhow::bail!("Source '{}' does not exist", pattern.display());
            }
            if !pattern.is_file() {
                anyhow::bail!("Source '{}' is not a file", pattern.display());
            }
            files.push(pattern.clone());
        }
    }

    // Warn about glob errors even if we found some files
    if !glob_errors.is_empty() {
        eprintln!(
            "Warning: Some paths could not be accessed during glob expansion:\n  {}",
            glob_errors.join("\n  ")
        );
    }

    // Note: Empty sources are handled upstream (args.paths.len() < 2 check),
    // so resolve_sources is never called with an empty slice.

    Ok(files)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle shell setup (doesn't require sources/destination)
    if args.shell_setup {
        return setup_shell_integration();
    }

    // Validate flag combinations
    if args.rm && args.no_verify {
        anyhow::bail!("Cannot use --rm with --no-verify: verification is required to safely remove source files.");
    }

    // Parse paths: all but last are sources, last is destination
    if args.paths.len() < 2 {
        anyhow::bail!("Usage: prcp <source>... <destination>\n\nAt least one source and a destination are required.");
    }

    let (source_paths, destination) = args.paths.split_at(args.paths.len() - 1);
    let destination = destination[0].clone();
    let source_paths: Vec<PathBuf> = source_paths.to_vec();

    // Resolve all source files (handles glob patterns)
    let sources = resolve_sources(&source_paths)?;
    let total_files = sources.len();

    // Validate destination for multi-file operations
    if total_files > 1 {
        // For multiple files, destination must be a directory
        // Check if exists and is NOT a directory (error case)
        if destination.exists() && !destination.is_dir() {
            anyhow::bail!(
                "Destination '{}' is not a directory (required for multiple source files)",
                destination.display()
            );
        }
        // Note: Directory creation is deferred until we're about to copy the first file
        // This avoids creating empty directories if all operations fail
    }

    // Warn about potentially dangerous combination
    if args.rm && args.continue_on_error && total_files > 1 && !args.yes {
        eprintln!("Warning: Using --rm with --continue-on-error may result in partial moves.");
        eprintln!("Some source files may be deleted while others remain if errors occur.");
        eprint!("Continue? (y/N): ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Operation cancelled");
            return Ok(());
        }
    }

    // Set up shutdown flag (can be reset if user declines cancellation)
    let shutdown = Arc::new(AtomicBool::new(false));

    // Set up key listener done flag (only set when operation is truly complete)
    let key_listener_done = Arc::new(AtomicBool::new(false));

    // Set up input active flag (pauses key listener while prompting user)
    let input_active = Arc::new(AtomicBool::new(false));

    // Set up pause/resume handling
    let paused = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let shutdown_key_listener = shutdown.clone();
    let done_key_listener = key_listener_done.clone();
    let input_active_key_listener = input_active.clone();

    // Set up terminal width watch channel for dynamic resize handling
    let (term_width_tx, term_width_rx) = watch::channel(get_terminal_width());

    // Spawn key listener task
    let key_task = task::spawn(async move {
        loop {
            // Only exit when operation is truly complete (not just when user pressed Ctrl+C)
            if done_key_listener.load(Ordering::SeqCst) {
                break;
            }

            // Skip event reading while user is being prompted for input
            // This prevents the key listener from consuming keystrokes meant for stdin
            if input_active_key_listener.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }

            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key_event)) => {
                        match key_event.code {
                            KeyCode::Char(' ') => {
                                let _ = tx.send(());
                            }
                            KeyCode::Char('c')
                                if key_event.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                // Just set the flag, don't break - user may decline cancellation
                                shutdown_key_listener.store(true, Ordering::SeqCst);
                            }
                            _ => {}
                        }
                    }
                    Ok(Event::Resize(width, _height)) => {
                        // Terminal was resized - broadcast the new width
                        let _ = term_width_tx.send(width);
                    }
                    _ => {}
                }
            }
        }
    });

    // Enable raw mode for keyboard input (uses RAII guard for safety)
    let mut raw_mode_guard = RawModeGuard::new();

    // Set up multi-progress display
    let multi = MultiProgress::new();

    // Overall progress bar (only shown for multiple files)
    let initial_width = *term_width_rx.borrow();
    let overall_pb = if total_files > 1 {
        let pb = multi.add(ProgressBar::new(total_files as u64));
        pb.set_style(
            create_overall_style(initial_width)?
                .progress_chars(PROGRESS_CHARS),
        );
        pb.set_prefix("Files");
        Some(pb)
    } else {
        None
    };

    // Track last terminal width for resize detection
    let mut last_overall_width = initial_width;

    // Track failures for --continue-on-error mode
    let mut failures: Vec<(PathBuf, String)> = Vec::new();
    let mut successful_copies = 0u64;
    let mut total_bytes_copied = 0u64;
    let mut total_copy_duration = Duration::ZERO;
    let mut total_verify_duration = Duration::ZERO;

    // Copy each file
    for source in &sources {
        // Check for terminal resize and update overall progress bar style
        let current_width = *term_width_rx.borrow();
        if current_width != last_overall_width {
            last_overall_width = current_width;
            if let Some(ref pb) = overall_pb {
                if let Ok(style) = create_overall_style(current_width) {
                    pb.set_style(style.progress_chars(PROGRESS_CHARS));
                }
            }
        }

        // Check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("\nCopy cancelled by user");
            break;
        }

        // Resolve destination path
        let dest_path = if destination.is_dir() {
            let filename = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Source '{}' has no filename", source.display()))?;
            destination.join(filename)
        } else {
            destination.clone()
        };

        // Get file metadata
        let metadata = match fs::metadata(source) {
            Ok(m) => m,
            Err(e) => {
                let error_msg = format!("Failed to read metadata: {}", e);
                if args.continue_on_error {
                    failures.push((source.clone(), error_msg));
                    if let Some(ref pb) = overall_pb {
                        pb.inc(1);
                    }
                    continue;
                } else {
                    anyhow::bail!("Failed to read metadata for '{}': {}", source.display(), e);
                }
            }
        };
        let file_size = metadata.len();

        // Check if destination exists
        if dest_path.exists() {
            let should_overwrite = if args.yes {
                true
            } else {
                // Pause key listener while prompting
                input_active.store(true, Ordering::SeqCst);

                eprint!(
                    "\nDestination '{}' already exists. Overwrite? (y/N): ",
                    dest_path.display()
                );
                io::stderr().flush()?;

                // Temporarily disable raw mode for input (guard ensures restoration)
                raw_mode_guard.disable_temporarily();

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                // Re-enable raw mode and resume key listener
                raw_mode_guard.restore();
                input_active.store(false, Ordering::SeqCst);

                input.trim().eq_ignore_ascii_case("y")
            };

            if !should_overwrite {
                let error_msg = "Skipped (destination exists)".to_string();
                if args.continue_on_error || total_files > 1 {
                    failures.push((source.clone(), error_msg));
                    if let Some(ref pb) = overall_pb {
                        pb.inc(1);
                    }
                    continue;
                } else {
                    println!("Copy cancelled");
                    break;
                }
            }
        }

        // Create parent directories if needed
        if let Some(parent) = dest_path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                let error_msg = format!("Failed to create directory: {}", e);
                if args.continue_on_error {
                    failures.push((source.clone(), error_msg));
                    if let Some(ref pb) = overall_pb {
                        pb.inc(1);
                    }
                    continue;
                } else {
                    anyhow::bail!(
                        "Failed to create destination directory for '{}': {}",
                        dest_path.display(),
                        e
                    );
                }
            }
        }

        // Create per-file progress bar
        let file_pb = multi.add(ProgressBar::new(file_size));
        let filename = source
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| source.display().to_string());

        // Set initial style using current terminal width
        let terminal_width = *term_width_rx.borrow();
        file_pb.set_style(
            create_copy_style(&filename, terminal_width)?
                .progress_chars(PROGRESS_CHARS),
        );

        // Perform the copy
        let result = copy_with_progress(
            source,
            &dest_path,
            file_size,
            args.buffer_size,
            &file_pb,
            &filename,
            paused.clone(),
            shutdown.clone(),
            input_active.clone(),
            &mut rx,
            &term_width_rx,
        )
        .await;

        file_pb.finish_and_clear();
        multi.remove(&file_pb);

        match result {
            Ok(copy_result) => {
                successful_copies += 1;
                total_bytes_copied += copy_result.bytes_copied;
                total_copy_duration += copy_result.copy_duration;

                // Build stats for this file
                let copy_speed = format_speed(copy_result.bytes_copied, copy_result.copy_duration);
                let copy_time = format_duration(copy_result.copy_duration);

                // Verify by default (unless --no-verify)
                // This loop allows retrying verification if the user cancels but declines deletion
                let verify_outcome = if !args.no_verify {
                    loop {
                        match verify_destination(&dest_path, &copy_result.source_hash, &multi, &shutdown, &term_width_rx) {
                            Ok(verify_result) => {
                                total_verify_duration += verify_result.verify_duration;
                                let verify_speed = format_speed(copy_result.bytes_copied, verify_result.verify_duration);
                                let verify_time = format_duration(verify_result.verify_duration);
                                break VerifyOutcome::Passed {
                                    speed: verify_speed,
                                    time: verify_time,
                                };
                            }
                            Err(VerifyError::Cancelled) => {
                                // User pressed Ctrl+C during verification - prompt for confirmation
                                // Pause key listener while prompting
                                input_active.store(true, Ordering::SeqCst);

                                eprint!(
                                    "\nVerification cancelled. Delete destination file '{}'? (y/N): ",
                                    dest_path.display()
                                );
                                io::stderr().flush()?;

                                // Temporarily disable raw mode for input
                                raw_mode_guard.disable_temporarily();

                                let mut input = String::new();
                                io::stdin().read_line(&mut input)?;

                                // Re-enable raw mode and resume key listener
                                raw_mode_guard.restore();
                                input_active.store(false, Ordering::SeqCst);

                                // Reset shutdown flag to allow continuing
                                shutdown.store(false, Ordering::SeqCst);

                                if input.trim().eq_ignore_ascii_case("y") {
                                    // Delete the unverified file
                                    if let Err(e) = fs::remove_file(&dest_path) {
                                        eprintln!("Warning: Failed to remove destination file: {}", e);
                                    }
                                    break VerifyOutcome::Failed;
                                }
                                // User declined deletion - retry verification
                                continue;
                            }
                            Err(VerifyError::Failed(error_msg)) => {
                                eprintln!("\n{}", error_msg);
                                if args.continue_on_error {
                                    failures.push((source.clone(), error_msg));
                                } else {
                                    anyhow::bail!("{}", error_msg);
                                }
                                break VerifyOutcome::Failed;
                            }
                        }
                    }
                } else {
                    VerifyOutcome::Skipped
                };

                // Track whether removal should proceed
                let mut removed = false;

                // Handle --rm flag: remove source only if verification passed or skipped
                let should_allow_removal = matches!(verify_outcome, VerifyOutcome::Passed { .. } | VerifyOutcome::Skipped);
                if args.rm && should_allow_removal {
                    if let Err(error_msg) = remove_source(source) {
                        eprintln!("\n{}", error_msg);
                        if args.continue_on_error {
                            failures.push((source.clone(), error_msg));
                        } else {
                            anyhow::bail!("{}", error_msg);
                        }
                    } else {
                        removed = true;
                    }
                }

                // Output results for single file
                if total_files == 1 {
                    // Status indicator based on verify outcome
                    let status = match &verify_outcome {
                        VerifyOutcome::Failed => "fail".red(),
                        VerifyOutcome::Passed { .. } | VerifyOutcome::Skipped if args.rm && !removed => {
                            "warn".yellow()
                        }
                        _ => "ok".green(),
                    };

                    match verify_outcome {
                        VerifyOutcome::Passed { speed: verify_speed, time: verify_time } => {
                            println!(
                                "\n[{}] Copied {} to '{}' (copy: {} @ {}, verify: {} @ {})",
                                status,
                                HumanBytes(copy_result.bytes_copied),
                                dest_path.display(),
                                copy_time,
                                copy_speed,
                                verify_time,
                                verify_speed
                            );
                        }
                        VerifyOutcome::Skipped | VerifyOutcome::Failed => {
                            println!(
                                "\n[{}] Copied {} to '{}' (copy: {} @ {})",
                                status,
                                HumanBytes(copy_result.bytes_copied),
                                dest_path.display(),
                                copy_time,
                                copy_speed
                            );
                        }
                    }

                    if args.rm && removed {
                        println!("Source file removed.");
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                if args.continue_on_error {
                    failures.push((source.clone(), error_msg));
                } else {
                    // Clean up and bail (raw mode cleaned up by RawModeGuard on drop)
                    key_listener_done.store(true, Ordering::SeqCst);
                    drop(raw_mode_guard); // Explicitly drop to restore terminal before cleanup
                    if let Some(ref pb) = overall_pb {
                        pb.finish_and_clear();
                    }
                    let _ = key_task.await;
                    anyhow::bail!("Failed to copy '{}': {}", source.display(), e);
                }
            }
        }

        // Update overall progress
        if let Some(ref pb) = overall_pb {
            pb.inc(1);
        }
    }

    // Signal key listener to stop
    key_listener_done.store(true, Ordering::SeqCst);

    // Restore terminal state (drop the guard to disable raw mode)
    drop(raw_mode_guard);

    // Finish overall progress bar
    if let Some(pb) = overall_pb {
        pb.finish_and_clear();
    }

    // Wait for key task to finish
    let _ = key_task.await;

    // Print summary for multiple files
    if total_files > 1 {
        if successful_copies > 0 {
            let copy_speed = format_speed(total_bytes_copied, total_copy_duration);
            let copy_time = format_duration(total_copy_duration);

            // Show verify stats only if verification was enabled AND at least one
            // verification succeeded. If verification was enabled but all failed,
            // total_verify_duration remains ZERO and we skip verify stats (the per-file
            // output already showed the failures).
            if !args.no_verify && total_verify_duration > Duration::ZERO {
                let verify_speed = format_speed(total_bytes_copied, total_verify_duration);
                let verify_time = format_duration(total_verify_duration);
                println!(
                    "\n{} Copied {} of {} files ({}, copy: {} @ {}, verify: {} @ {})",
                    "Summary:".bold(),
                    successful_copies,
                    total_files,
                    HumanBytes(total_bytes_copied),
                    copy_time,
                    copy_speed,
                    verify_time,
                    verify_speed
                );
                if args.rm {
                    println!("Source files removed after verification.");
                }
            } else {
                // Either --no-verify was used, or all verifications failed
                println!(
                    "\n{} Copied {} of {} files ({}, {} @ {})",
                    "Summary:".bold(),
                    successful_copies,
                    total_files,
                    HumanBytes(total_bytes_copied),
                    copy_time,
                    copy_speed
                );
            }
        } else {
            // All copies failed - show summary without timing stats
            println!(
                "\n{} Copied 0 of {} files",
                "Summary:".bold(),
                total_files
            );
        }
    }

    // Report failures
    if !failures.is_empty() {
        eprintln!("\nFailures ({}):", failures.len());
        for (path, error) in &failures {
            eprintln!("  {}: {}", path.display(), error);
        }
        anyhow::bail!("{} file(s) failed to copy", failures.len());
    }

    Ok(())
}

/// Result of a copy operation, including bytes copied, source hash, and timing
struct CopyResult {
    bytes_copied: u64,
    source_hash: blake3::Hash,
    copy_duration: Duration,
}

/// Result of a successful verification, including timing
struct VerifyResult {
    verify_duration: Duration,
}

/// Error types for verification
enum VerifyError {
    /// Verification was cancelled by user (Ctrl+C)
    Cancelled,
    /// Verification failed with an error message
    Failed(String),
}

/// Outcome of verification for a single file
enum VerifyOutcome {
    /// Verification was performed and succeeded, includes timing stats for display
    Passed {
        speed: String,
        time: String,
    },
    /// Verification was skipped (--no-verify flag)
    Skipped,
    /// Verification was performed but failed
    Failed,
}

/// Hint to the OS for optimal sequential I/O performance.
/// On macOS: disables page cache (F_NOCACHE) for large sequential transfers.
/// On Linux: enables sequential read-ahead (posix_fadvise).
/// This is a best-effort optimization - failures are silently ignored.
#[cfg(target_os = "macos")]
fn hint_sequential_io(file: &File) {
    // F_NOCACHE = 48 on macOS
    const F_NOCACHE: libc::c_int = 48;
    let fd = file.as_raw_fd();
    // Ignore result - this is purely an optimization hint
    let _ = unsafe { libc::fcntl(fd, F_NOCACHE, 1) };
}

/// Hint to the OS for optimal sequential I/O performance.
/// On macOS: disables page cache (F_NOCACHE) for large sequential transfers.
/// On Linux: enables sequential read-ahead (posix_fadvise).
/// This is a best-effort optimization - failures are silently ignored.
#[cfg(target_os = "linux")]
fn hint_sequential_io(file: &File) {
    let fd = file.as_raw_fd();
    // POSIX_FADV_SEQUENTIAL = 2: expect sequential access
    // Ignore result - this is purely an optimization hint
    let _ = unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_SEQUENTIAL) };
}

/// Hint to the OS for optimal sequential I/O performance.
/// No-op on platforms without specific sequential I/O hints.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn hint_sequential_io(_file: &File) {
    // No-op on other platforms
}

/// Pre-allocate space for a file to help filesystem allocate contiguous blocks.
/// On Linux: uses fallocate for faster allocation.
/// On other platforms: uses set_len as a fallback.
/// This is a best-effort optimization - failures are silently ignored.
#[cfg(target_os = "linux")]
fn try_preallocate(file: &File, size: u64) {
    let fd = file.as_raw_fd();
    // Use fallocate for faster pre-allocation on Linux
    let result = unsafe { libc::fallocate(fd, 0, 0, size as libc::off_t) };
    if result == -1 {
        // Fall back to set_len if fallocate fails (e.g., unsupported filesystem)
        let _ = file.set_len(size);
    }
}

/// Pre-allocate space for a file to help filesystem allocate contiguous blocks.
/// On Linux: uses fallocate for faster allocation.
/// On other platforms: uses set_len as a fallback.
/// This is a best-effort optimization - failures are silently ignored.
#[cfg(not(target_os = "linux"))]
fn try_preallocate(file: &File, size: u64) {
    let _ = file.set_len(size);
}

/// Check if source and destination are on the same physical device.
/// Used to decide between parallel (different devices) and sequential (same device) I/O.
/// On spinning HDDs, parallel read/write to the same device causes head thrashing.
#[cfg(unix)]
fn same_device(source: &Path, destination: &Path) -> bool {
    let src_meta = match fs::metadata(source) {
        Ok(m) => m,
        Err(_) => return false,
    };

    // For destination, check the parent directory since the file may not exist yet
    let dst_dir = destination.parent().unwrap_or(destination);
    let dst_meta = match fs::metadata(dst_dir) {
        Ok(m) => m,
        Err(_) => return false,
    };

    src_meta.dev() == dst_meta.dev()
}

#[cfg(not(unix))]
fn same_device(_source: &Path, _destination: &Path) -> bool {
    // On non-Unix platforms, assume same device (safer, uses sequential I/O)
    true
}

/// Calculate Blake3 hash of a file with optional progress indicator, cancellation, and resize support.
///
/// Uses buffered I/O with parallel hashing for large buffers. This provides
/// good performance while allowing progress updates for very large files.
///
/// If `shutdown` is provided and becomes true during calculation, returns an error
/// indicating cancellation. This allows the caller to handle Ctrl+C gracefully.
///
/// If `term_width_rx` and `filename` are provided, the progress bar style will be
/// updated when the terminal is resized.
fn calculate_file_hash_with_progress(
    path: &Path,
    pb: Option<&ProgressBar>,
    shutdown: Option<&Arc<AtomicBool>>,
    term_width_rx: Option<&watch::Receiver<u16>>,
    filename: Option<&str>,
) -> Result<blake3::Hash> {
    let file = File::open(path).context("Failed to open file for hash verification")?;
    hint_sequential_io(&file);

    let mut reader = io::BufReader::with_capacity(DEFAULT_BUFFER_SIZE, file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; DEFAULT_BUFFER_SIZE];
    let mut bytes_hashed = 0u64;

    // Track last terminal width for resize detection
    let mut last_width = term_width_rx.map(|rx| *rx.borrow());

    loop {
        // Check for terminal resize and update progress bar style
        if let (Some(rx), Some(progress), Some(fname), Some(prev_width)) =
            (term_width_rx, pb, filename, last_width.as_mut())
        {
            let current_width = *rx.borrow();
            if current_width != *prev_width {
                *prev_width = current_width;
                if let Ok(style) = create_verify_style(fname, current_width) {
                    progress.set_style(style.progress_chars(PROGRESS_CHARS));
                }
            }
        }

        // Check for cancellation before each read
        if let Some(shutdown_flag) = shutdown {
            if shutdown_flag.load(Ordering::SeqCst) {
                anyhow::bail!("Verification cancelled by user");
            }
        }

        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        // Use parallel hashing for large chunks
        if bytes_read >= PARALLEL_HASH_THRESHOLD {
            hasher.update_rayon(&buffer[..bytes_read]);
        } else {
            hasher.update(&buffer[..bytes_read]);
        }

        bytes_hashed += bytes_read as u64;
        if let Some(progress) = pb {
            progress.set_position(bytes_hashed);
        }
    }

    Ok(hasher.finalize())
}

/// Calculate Blake3 hash of a file (without progress tracking).
/// Convenience wrapper for cases where progress isn't needed.
#[allow(dead_code)]
fn calculate_file_hash(path: &Path) -> Result<blake3::Hash> {
    calculate_file_hash_with_progress(path, None, None, None, None)
}

/// Escape braces in a string for use in indicatif templates.
/// Indicatif uses `{placeholder}` syntax, so literal braces must be doubled.
fn escape_template_braces(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
}

/// Get the current terminal width, with a fallback default.
fn get_terminal_width() -> u16 {
    crossterm::terminal::size()
        .map(|(w, _)| w)
        .unwrap_or(80) // Default to 80 columns if detection fails
}

/// Calculate the progress bar width based on terminal width and fixed element overhead.
///
/// The bar width is calculated by subtracting the overhead from the terminal width,
/// then clamping to a reasonable range (min 10, max 100 characters).
fn calculate_bar_width(terminal_width: u16, fixed_overhead: u16) -> u16 {
    const MIN_BAR_WIDTH: u16 = 10;
    const MAX_BAR_WIDTH: u16 = 100;

    let available = terminal_width.saturating_sub(fixed_overhead);
    available.clamp(MIN_BAR_WIDTH, MAX_BAR_WIDTH)
}

/// Create a progress style for the overall files progress bar.
fn create_overall_style(terminal_width: u16) -> Result<ProgressStyle> {
    // "Files [bar] 999/999" = ~20 chars overhead
    let bar_width = calculate_bar_width(terminal_width, 20);
    ProgressStyle::default_bar()
        .template(&format!(
            "{{prefix:.bold}} [{{bar:{}.green/dim}}] {{pos}}/{{len}}",
            bar_width
        ))
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Create a progress style for the copy progress bar.
fn create_copy_style(filename: &str, terminal_width: u16) -> Result<ProgressStyle> {
    let safe_filename = escape_template_braces(filename);
    // spinner(2) + filename + brackets(4) + bytes(25) + speed/eta(25) + spaces(3) = ~60 + filename.len()
    #[allow(clippy::cast_possible_truncation)]
    let filename_len = filename.len().min(u16::MAX as usize) as u16;
    let overhead = 60 + filename_len;
    let bar_width = calculate_bar_width(terminal_width, overhead);

    ProgressStyle::default_bar()
        .template(&format!(
            "{{spinner:.green}} {} [{{bar:{}.cyan/blue}}] {}",
            safe_filename, bar_width, PROGRESS_STATS_FORMAT
        ))
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Create a progress style for the verification progress bar.
fn create_verify_style(filename: &str, terminal_width: u16) -> Result<ProgressStyle> {
    let safe_filename = escape_template_braces(filename);
    // spinner(2) + filename + brackets(4) + bytes(25) + speed/eta(25) + " verifying"(10) + spaces(3) = ~70 + filename.len()
    #[allow(clippy::cast_possible_truncation)]
    let filename_len = filename.len().min(u16::MAX as usize) as u16;
    let overhead = 70 + filename_len;
    let bar_width = calculate_bar_width(terminal_width, overhead);

    ProgressStyle::default_bar()
        .template(&format!(
            "{{spinner:.yellow}} {} [{{bar:{}.yellow/dim}}] {} verifying",
            safe_filename, bar_width, PROGRESS_STATS_FORMAT
        ))
        .map_err(|e| anyhow::anyhow!("{}", e))
}

/// Format a duration in a human-readable way (e.g., "1.23s", "45.6ms")
fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs >= 1.0 {
        format!("{:.2}s", secs)
    } else {
        format!("{:.1}ms", secs * 1000.0)
    }
}

/// Calculate and format transfer speed as bytes per second.
///
/// Handles edge cases including zero duration and potential overflow
/// when converting from f64 to u64.
#[allow(clippy::cast_precision_loss)] // Precision loss is acceptable for human-readable display
#[allow(clippy::cast_possible_truncation)] // Truncation is expected - we want integer bytes/sec
#[allow(clippy::cast_sign_loss)] // Result is always positive (bytes/secs where both > 0)
fn format_speed(bytes: u64, duration: Duration) -> String {
    let secs = duration.as_secs_f64();
    if secs > 0.0 {
        let bytes_per_sec = (bytes as f64 / secs).min(u64::MAX as f64) as u64;
        format!("{}/s", HumanBytes(bytes_per_sec))
    } else {
        "N/A".to_string()
    }
}

/// Verify that the destination file matches the expected hash.
///
/// This function performs Blake3 hash verification to ensure the copy was
/// successful. Shows a progress bar during verification for large files.
/// Supports cancellation via `shutdown` flag and terminal resize via `term_width_rx`.
///
/// Returns `Ok(VerifyResult)` on success, or `Err(VerifyError)` if verification
/// fails or is cancelled.
fn verify_destination(
    destination: &Path,
    expected_hash: &blake3::Hash,
    multi: &MultiProgress,
    shutdown: &Arc<AtomicBool>,
    term_width_rx: &watch::Receiver<u16>,
) -> Result<VerifyResult, VerifyError> {
    let start_time = Instant::now();

    // Get file size for progress bar
    let file_size = fs::metadata(destination)
        .map_err(|e| VerifyError::Failed(format!("Failed to get destination metadata: {}", e)))?
        .len();

    // Get filename for display
    let filename = destination
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| destination.display().to_string());

    // Create verification progress bar with dynamic width
    let terminal_width = *term_width_rx.borrow();
    let pb = multi.add(ProgressBar::new(file_size));
    pb.set_style(
        create_verify_style(&filename, terminal_width)
            .map_err(|e| VerifyError::Failed(format!("Failed to create progress bar: {}", e)))?
            .progress_chars(PROGRESS_CHARS),
    );

    // Calculate destination hash with progress, cancellation, and resize support
    let dest_hash = calculate_file_hash_with_progress(
        destination,
        Some(&pb),
        Some(shutdown),
        Some(term_width_rx),
        Some(&filename),
    )
    .map_err(|e| {
        pb.finish_and_clear();
        let err_msg = e.to_string();
        if err_msg.contains("cancelled") {
            VerifyError::Cancelled
        } else {
            VerifyError::Failed(format!("Failed to verify destination: {}", e))
        }
    })?;

    pb.finish_and_clear();
    multi.remove(&pb);

    // Verify hashes match
    if expected_hash != &dest_hash {
        return Err(VerifyError::Failed(format!(
            "Hash mismatch!\n  Expected: {}\n  Got:      {}",
            expected_hash.to_hex(),
            dest_hash.to_hex()
        )));
    }

    Ok(VerifyResult {
        verify_duration: start_time.elapsed(),
    })
}

/// Remove the source file after successful verification.
fn remove_source(source: &Path) -> Result<(), String> {
    fs::remove_file(source)
        .map_err(|e| format!("Failed to remove source '{}': {}", source.display(), e))
}

/// Message sent from reader thread to writer thread (used in parallel copy mode)
enum CopyMessage {
    /// Data chunk to write (buffer, bytes_read).
    /// Buffer capacity is preserved to avoid reallocation when returned to pool.
    Data(Vec<u8>, usize),
    /// End of file reached
    Eof,
    /// Error occurred during read
    Error(String),
}

/// Dispatches to the appropriate copy strategy based on whether source and destination
/// are on the same physical device.
/// - Same device (e.g., spinning HDD): uses sequential I/O to avoid head thrashing
/// - Different devices (e.g., SSD to SSD, or cross-device): uses parallel I/O for throughput
async fn copy_with_progress(
    source: &Path,
    destination: &Path,
    file_size: u64,
    buffer_size: usize,
    pb: &ProgressBar,
    filename: &str,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    input_active: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
    _term_width_rx: &watch::Receiver<u16>,
) -> Result<CopyResult> {
    if same_device(source, destination) {
        copy_sequential(source, destination, file_size, buffer_size, pb, filename, paused, shutdown, input_active, rx).await
    } else {
        copy_parallel(source, destination, file_size, buffer_size, pb, filename, paused, shutdown, input_active, rx).await
    }
}

/// Sequential copy for same-device scenarios (optimal for spinning HDDs).
/// Performs read→hash→write in sequence to avoid disk head thrashing.
async fn copy_sequential(
    source: &Path,
    destination: &Path,
    file_size: u64,
    buffer_size: usize,
    pb: &ProgressBar,
    filename: &str,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    input_active: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    let start_time = Instant::now();
    let mut src_file = File::open(source).context("Failed to open source file")?;
    let dst_file = File::create(destination).context("Failed to create destination file")?;
    hint_sequential_io(&src_file);
    hint_sequential_io(&dst_file);

    // Pre-allocate destination file to help filesystem allocate contiguous blocks.
    // Uses fallocate on Linux for faster allocation.
    // Ignore errors - pre-allocation is an optimization, not a requirement.
    try_preallocate(&dst_file, file_size);

    // Use RAII guard to ensure partial file cleanup on any error path
    // (Ctrl+C, I/O errors, etc.). The guard is defused on successful completion.
    let mut guard = PartialFileGuard::new(destination, dst_file);

    let mut buffer = vec![0u8; buffer_size];
    let mut total_bytes = 0u64;
    let mut hasher = blake3::Hasher::new();

    loop {
        // Check for shutdown - prompt user for confirmation
        if shutdown.load(Ordering::SeqCst) {
            // Pause key listener while prompting
            input_active.store(true, Ordering::SeqCst);

            // Clear progress bar line and prompt user
            pb.set_message("");
            eprint!(
                "\nCancel copy of '{}'? Partial file will be deleted. (y/N): ",
                filename
            );
            let _ = io::stderr().flush();

            // Temporarily disable raw mode for input
            let _ = crossterm::terminal::disable_raw_mode();

            let mut input = String::new();
            let read_result = io::stdin().read_line(&mut input);

            // Re-enable raw mode and resume key listener
            let _ = crossterm::terminal::enable_raw_mode();
            input_active.store(false, Ordering::SeqCst);

            if read_result.is_ok() && input.trim().eq_ignore_ascii_case("y") {
                return Err(anyhow::anyhow!("Copy cancelled by user"));
            }

            // User declined cancellation - reset shutdown flag and continue
            shutdown.store(false, Ordering::SeqCst);
        }

        // Check for pause toggle
        if rx.try_recv().is_ok() {
            let was_paused = paused.fetch_xor(true, Ordering::SeqCst);
            if !was_paused {
                pb.set_message("PAUSED - Press space to resume");
            } else {
                pb.set_message("");
            }
        }

        // Wait while paused
        while paused.load(Ordering::SeqCst) {
            // Check for shutdown while paused - prompt user for confirmation
            if shutdown.load(Ordering::SeqCst) {
                // Pause key listener while prompting
                input_active.store(true, Ordering::SeqCst);

                pb.set_message("");
                eprint!(
                    "\nCancel copy of '{}'? Partial file will be deleted. (y/N): ",
                    filename
                );
                let _ = io::stderr().flush();

                // Temporarily disable raw mode for input
                let _ = crossterm::terminal::disable_raw_mode();

                let mut input = String::new();
                let read_result = io::stdin().read_line(&mut input);

                // Re-enable raw mode and resume key listener
                let _ = crossterm::terminal::enable_raw_mode();
                input_active.store(false, Ordering::SeqCst);

                if read_result.is_ok() && input.trim().eq_ignore_ascii_case("y") {
                    return Err(anyhow::anyhow!("Copy cancelled by user"));
                }

                // User declined cancellation - reset shutdown flag and continue
                shutdown.store(false, Ordering::SeqCst);
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            if rx.try_recv().is_ok() {
                paused.store(false, Ordering::SeqCst);
                pb.set_message("");
            }
        }

        // Read from source
        let bytes_read = match src_file.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) => return Err(e.into()),
        };

        // Hash (sequential copy - no head thrashing on same device)
        // Use parallel hashing only for large chunks where it's beneficial
        if bytes_read >= PARALLEL_HASH_THRESHOLD {
            hasher.update_rayon(&buffer[..bytes_read]);
        } else {
            hasher.update(&buffer[..bytes_read]);
        }

        // Write to destination
        guard
            .file_mut()
            .write_all(&buffer[..bytes_read])
            .context("Failed to write to destination file")?;

        total_bytes += bytes_read as u64;
        pb.set_position(total_bytes);
    }

    // Defuse the guard and get the file back for final operations
    // From this point on, the file will NOT be cleaned up on error
    let mut dst_file = guard.defuse();

    // Ensure all data is written and synced to disk
    dst_file
        .flush()
        .context("Failed to flush destination file")?;
    dst_file
        .sync_all()
        .context("Failed to sync destination file to disk")?;

    // Copy file permissions
    let metadata = fs::metadata(source)?;
    fs::set_permissions(destination, metadata.permissions())?;

    drop(dst_file);

    Ok(CopyResult {
        bytes_copied: total_bytes,
        source_hash: hasher.finalize(),
        copy_duration: start_time.elapsed(),
    })
}

/// Parallel copy for cross-device scenarios (optimal for SSDs/NVMe).
/// Uses dedicated reader/writer threads for true parallelism with buffer pooling.
/// - Reader thread: reads into pooled buffers, sends to channel
/// - Writer task: receives from channel, hashes, writes to disk, returns buffers
/// - Main async task: handles pause/resume/shutdown coordination
async fn copy_parallel(
    source: &Path,
    destination: &Path,
    file_size: u64,
    buffer_size: usize,
    pb: &ProgressBar,
    filename: &str,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    input_active: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    let start_time = Instant::now();
    let src_path = source.to_path_buf();
    let dst_path = destination.to_path_buf();

    // Open files with sequential access hints
    let src_file = File::open(&src_path).context("Failed to open source file")?;
    let dst_file = File::create(&dst_path).context("Failed to create destination file")?;
    hint_sequential_io(&src_file);
    hint_sequential_io(&dst_file);

    // Pre-allocate destination file to help filesystem allocate contiguous blocks.
    // Uses fallocate on Linux for faster allocation.
    // Ignore errors - pre-allocation is an optimization, not a requirement.
    try_preallocate(&dst_file, file_size);

    // Use RAII guard to ensure partial file cleanup on any error path
    // (Ctrl+C, I/O errors, etc.). The guard is defused on successful completion.
    let mut guard = PartialFileGuard::new(&dst_path, dst_file);

    // Channel for passing data from reader to writer (bounded to limit memory).
    // Higher depth allows more read-ahead for fast storage.
    let (data_tx, data_rx) = std::sync::mpsc::sync_channel::<CopyMessage>(CHANNEL_DEPTH);

    // Buffer pool: writer returns used buffers to reader for reuse.
    // This eliminates per-chunk allocation overhead.
    let (buffer_return_tx, buffer_return_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(CHANNEL_DEPTH);

    // Pre-allocate buffer pool
    for _ in 0..CHANNEL_DEPTH {
        let _ = buffer_return_tx.send(vec![0u8; buffer_size]);
    }

    // Shared state for coordination
    let reader_paused = Arc::clone(&paused);
    let reader_shutdown = Arc::clone(&shutdown);

    // Spawn reader thread
    let reader_handle = std::thread::spawn(move || {
        let mut file = src_file;

        loop {
            // Check for shutdown
            if reader_shutdown.load(Ordering::SeqCst) {
                let _ = data_tx.send(CopyMessage::Error("Cancelled".to_string()));
                return;
            }

            // Wait while paused
            while reader_paused.load(Ordering::SeqCst) {
                if reader_shutdown.load(Ordering::SeqCst) {
                    let _ = data_tx.send(CopyMessage::Error("Cancelled".to_string()));
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }

            // Get a buffer from the pool (or allocate if pool exhausted)
            let mut buffer = buffer_return_rx
                .recv_timeout(Duration::from_millis(100))
                .unwrap_or_else(|_| vec![0u8; buffer_size]);

            // Read chunk
            match file.read(&mut buffer) {
                Ok(0) => {
                    let _ = data_tx.send(CopyMessage::Eof);
                    return;
                }
                Ok(n) => {
                    // Send buffer with bytes_read - preserves buffer capacity for reuse
                    if data_tx.send(CopyMessage::Data(buffer, n)).is_err() {
                        return; // Receiver dropped, exit
                    }
                }
                Err(e) => {
                    let _ = data_tx.send(CopyMessage::Error(e.to_string()));
                    return;
                }
            }
        }
    });

    // Writer runs in the main task context, receiving from channel
    let mut hasher = blake3::Hasher::new();
    let mut total_bytes = 0u64;

    loop {
        // Check for pause toggle
        if rx.try_recv().is_ok() {
            let was_paused = paused.fetch_xor(true, Ordering::SeqCst);
            if !was_paused {
                pb.set_message("PAUSED - Press space to resume");
            } else {
                pb.set_message("");
            }
        }

        // Non-blocking check for shutdown - prompt user for confirmation
        if shutdown.load(Ordering::SeqCst) {
            // Pause key listener while prompting
            input_active.store(true, Ordering::SeqCst);

            // Clear progress bar line and prompt user
            pb.set_message("");
            eprint!(
                "\nCancel copy of '{}'? Partial file will be deleted. (y/N): ",
                filename
            );
            let _ = io::stderr().flush();

            // Temporarily disable raw mode for input
            let _ = crossterm::terminal::disable_raw_mode();

            let mut input = String::new();
            let read_result = io::stdin().read_line(&mut input);

            // Re-enable raw mode and resume key listener
            let _ = crossterm::terminal::enable_raw_mode();
            input_active.store(false, Ordering::SeqCst);

            if read_result.is_ok() && input.trim().eq_ignore_ascii_case("y") {
                // Wait for reader to notice and exit
                let _ = reader_handle.join();
                return Err(anyhow::anyhow!("Copy cancelled by user"));
            }

            // User declined cancellation - reset shutdown flag and continue
            shutdown.store(false, Ordering::SeqCst);
        }

        // Wait while paused (but keep checking for shutdown/unpause)
        while paused.load(Ordering::SeqCst) {
            if shutdown.load(Ordering::SeqCst) {
                // Pause key listener while prompting
                input_active.store(true, Ordering::SeqCst);

                pb.set_message("");
                eprint!(
                    "\nCancel copy of '{}'? Partial file will be deleted. (y/N): ",
                    filename
                );
                let _ = io::stderr().flush();

                // Temporarily disable raw mode for input
                let _ = crossterm::terminal::disable_raw_mode();

                let mut input = String::new();
                let read_result = io::stdin().read_line(&mut input);

                // Re-enable raw mode and resume key listener
                let _ = crossterm::terminal::enable_raw_mode();
                input_active.store(false, Ordering::SeqCst);

                if read_result.is_ok() && input.trim().eq_ignore_ascii_case("y") {
                    let _ = reader_handle.join();
                    return Err(anyhow::anyhow!("Copy cancelled by user"));
                }

                // User declined cancellation - reset shutdown flag and continue
                shutdown.store(false, Ordering::SeqCst);
            }

            if rx.try_recv().is_ok() {
                paused.store(false, Ordering::SeqCst);
                pb.set_message("");
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Try to receive data with timeout to allow checking pause/shutdown
        match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(CopyMessage::Data(buffer, bytes_read)) => {
                let data = &buffer[..bytes_read];

                // Use parallel hashing only for large chunks where it's beneficial
                if bytes_read >= PARALLEL_HASH_THRESHOLD {
                    hasher.update_rayon(data);
                } else {
                    hasher.update(data);
                }

                guard
                    .file_mut()
                    .write_all(data)
                    .context("Failed to write to destination file")?;
                total_bytes += bytes_read as u64;
                pb.set_position(total_bytes);

                // Return buffer to pool for reuse (capacity is preserved since we used slice)
                let _ = buffer_return_tx.try_send(buffer);
            }
            Ok(CopyMessage::Eof) => {
                break;
            }
            Ok(CopyMessage::Error(e)) => {
                let _ = reader_handle.join();
                if e == "Cancelled" {
                    return Err(anyhow::anyhow!("Copy cancelled by user"));
                }
                return Err(anyhow::anyhow!("Read error: {}", e));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Continue loop to check pause/shutdown
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                // Reader thread exited unexpectedly
                let _ = reader_handle.join();
                return Err(anyhow::anyhow!("Reader thread disconnected unexpectedly"));
            }
        }
    }

    // Wait for reader thread to complete
    reader_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Reader thread panicked"))?;

    // Defuse the guard and get the file back for final operations
    // From this point on, the file will NOT be cleaned up on error
    let mut dst_file = guard.defuse();

    // Ensure all data is written and synced to disk
    dst_file
        .flush()
        .context("Failed to flush destination file")?;
    dst_file
        .sync_all()
        .context("Failed to sync destination file to disk")?;

    // Copy file permissions
    let metadata = fs::metadata(&src_path)?;
    fs::set_permissions(&dst_path, metadata.permissions())?;

    // Explicitly close the destination file
    drop(dst_file);

    Ok(CopyResult {
        bytes_copied: total_bytes,
        source_hash: hasher.finalize(),
        copy_duration: start_time.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    mod format_duration_tests {
        use super::*;

        #[test]
        fn zero_duration() {
            assert_eq!(format_duration(Duration::ZERO), "0.0ms");
        }

        #[test]
        fn sub_millisecond() {
            assert_eq!(format_duration(Duration::from_micros(500)), "0.5ms");
        }

        #[test]
        fn milliseconds() {
            assert_eq!(format_duration(Duration::from_millis(500)), "500.0ms");
        }

        #[test]
        fn one_second() {
            assert_eq!(format_duration(Duration::from_secs(1)), "1.00s");
        }

        #[test]
        fn seconds_with_decimal() {
            assert_eq!(format_duration(Duration::from_millis(1500)), "1.50s");
        }

        #[test]
        fn large_duration() {
            assert_eq!(format_duration(Duration::from_secs(3600)), "3600.00s");
        }
    }

    mod format_speed_tests {
        use super::*;

        #[test]
        fn zero_duration_returns_na() {
            // Our implementation returns "N/A" for zero duration
            assert_eq!(format_speed(1000, Duration::ZERO), "N/A");
        }

        #[test]
        fn zero_bytes() {
            // 0 bytes over any time is 0 B/s
            assert_eq!(format_speed(0, Duration::from_secs(1)), "0 B/s");
        }

        #[test]
        fn one_byte_per_second() {
            assert_eq!(format_speed(1, Duration::from_secs(1)), "1 B/s");
        }

        #[test]
        fn kilobytes_per_second() {
            // 1024 bytes in 1 second = 1 KiB/s
            assert_eq!(format_speed(1024, Duration::from_secs(1)), "1.00 KiB/s");
        }

        #[test]
        fn megabytes_per_second() {
            // 1 MiB in 1 second = 1 MiB/s
            let mib = 1024 * 1024;
            assert_eq!(format_speed(mib, Duration::from_secs(1)), "1.00 MiB/s");
        }

        #[test]
        fn gigabytes_per_second() {
            // 1 GiB in 1 second = 1 GiB/s
            let gib = 1024 * 1024 * 1024;
            assert_eq!(format_speed(gib, Duration::from_secs(1)), "1.00 GiB/s");
        }

        #[test]
        fn fractional_time() {
            // 1 MiB in 0.5 seconds = 2 MiB/s
            let mib = 1024 * 1024;
            assert_eq!(format_speed(mib, Duration::from_millis(500)), "2.00 MiB/s");
        }

        #[test]
        fn very_small_duration() {
            // Tests the overflow protection for very fast copies
            let result = format_speed(1024 * 1024 * 1024, Duration::from_nanos(1));
            // Should not panic, and should return something reasonable
            assert!(result.ends_with("/s"));
        }

        #[test]
        fn very_large_bytes() {
            // Test with u64::MAX bytes
            let result = format_speed(u64::MAX, Duration::from_secs(1));
            assert!(result.ends_with("/s"));
        }
    }
}
