// Clippy lints to catch common issues
#![warn(clippy::unwrap_used)] // Prefer explicit error handling
#![warn(clippy::expect_used)] // Prefer explicit error handling
#![warn(clippy::panic)] // Avoid panics in library code
#![deny(clippy::unimplemented)] // Don't leave unimplemented!() in code
#![warn(clippy::cast_possible_truncation)] // Warn on lossy casts (e.g., f64 to u64)
#![warn(clippy::cast_sign_loss)] // Warn when casting signed to unsigned
#![warn(clippy::cast_precision_loss)] // Warn when casting to float loses precision

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
// Blake3 imported via blake3 crate (no Digest trait needed)
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task;

/// Create a buffer without zeroing memory.
///
/// This is an optimization to avoid the cost of zeroing large buffers
/// when they will be immediately overwritten by file reads.
///
/// # Safety
/// The returned Vec has uninitialized contents. Callers MUST:
/// - Only read bytes that have been written to (e.g., by `File::read`)
/// - Never access uninitialized portions of the buffer
#[inline]
fn create_uninit_buffer(size: usize) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(size);
    // SAFETY: We're setting length to capacity. The memory is uninitialized,
    // but callers will only access bytes that File::read has written to.
    // File::read returns the number of bytes actually read, so we never
    // access uninitialized memory as long as we respect that count.
    #[allow(clippy::uninit_vec)]
    unsafe {
        buffer.set_len(size);
    }
    buffer
}

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

    /// Skip Blake3 verification after copy (not allowed with --rm)
    #[arg(long)]
    no_verify: bool,

    /// Continue copying remaining files if one fails
    #[arg(long)]
    continue_on_error: bool,

    /// Skip all confirmation prompts (assume yes)
    #[arg(long, short = 'y')]
    yes: bool,

    /// I/O buffer size (e.g., 16M, 64M, 1G). Default: 16M. Range: 4K-1G.
    #[arg(long, default_value = "16M", value_parser = parse_buffer_size)]
    buffer_size: usize,
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

/// Minimum allowed buffer size (4KB)
const MIN_BUFFER_SIZE: usize = 4 * 1024;
/// Maximum allowed buffer size (1GB)
const MAX_BUFFER_SIZE: usize = 1024 * 1024 * 1024;

/// Parse a human-readable buffer size string (e.g., "16M", "64M", "1G").
///
/// Supported suffixes:
/// - K/KB: kilobytes (1024 bytes)
/// - M/MB: megabytes (1024^2 bytes)
/// - G/GB: gigabytes (1024^3 bytes)
///
/// Returns an error if the value is outside the valid range (4KB - 1GB).
fn parse_buffer_size(s: &str) -> Result<usize, String> {
    let s = s.trim().to_uppercase();

    // Find where the numeric part ends and suffix begins
    let (num_str, suffix) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(idx) => s.split_at(idx),
        None => (s.as_str(), ""),
    };

    let base: usize = num_str
        .parse()
        .map_err(|_| format!("Invalid number: {}", num_str))?;

    let multiplier: usize = match suffix {
        "" => 1,
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        _ => return Err(format!("Unknown suffix: {}. Use K, M, or G.", suffix)),
    };

    let size = base
        .checked_mul(multiplier)
        .ok_or_else(|| "Buffer size overflow".to_string())?;

    if size < MIN_BUFFER_SIZE {
        return Err(format!(
            "Buffer size {} is below minimum (4KB)",
            format_buffer_size(size)
        ));
    }

    if size > MAX_BUFFER_SIZE {
        return Err(format!(
            "Buffer size {} exceeds maximum (1GB)",
            format_buffer_size(size)
        ));
    }

    Ok(size)
}

/// Format a buffer size for display (e.g., 16777216 -> "16MB")
fn format_buffer_size(size: usize) -> String {
    if size >= 1024 * 1024 * 1024 && size.is_multiple_of(1024 * 1024 * 1024) {
        format!("{}GB", size / (1024 * 1024 * 1024))
    } else if size >= 1024 * 1024 && size.is_multiple_of(1024 * 1024) {
        format!("{}MB", size / (1024 * 1024))
    } else if size >= 1024 && size.is_multiple_of(1024) {
        format!("{}KB", size / 1024)
    } else {
        format!("{} bytes", size)
    }
}

/// Progress bar characters for smooth progress visualization.
const PROGRESS_CHARS: &str = "█▉▊▋▌▍▎▏  ";

/// Shared format string for progress stats (bytes, percentage, speed, ETA).
const PROGRESS_STATS_FORMAT: &str = "{bytes}/{total_bytes} ({percent}%) ({bytes_per_sec}, {eta})";

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

            if event::poll(Duration::from_millis(250)).unwrap_or(false) {
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
    // Track last terminal width to detect resize
    let mut last_overall_width = *term_width_rx.borrow();
    let overall_pb = if total_files > 1 {
        let pb = multi.add(ProgressBar::new(total_files as u64));
        pb.set_style(
            create_overall_style(last_overall_width)?
                .progress_chars(PROGRESS_CHARS),
        );
        pb.set_prefix("Files");
        Some(pb)
    } else {
        None
    };

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
        let current_width = *term_width_rx.borrow();
        file_pb.set_style(
            create_copy_style(&filename, current_width)?
                .progress_chars(PROGRESS_CHARS),
        );

        // Perform the copy
        let result = copy_with_progress(
            source,
            &dest_path,
            &file_pb,
            &filename,
            paused.clone(),
            shutdown.clone(),
            input_active.clone(),
            &mut rx,
            &term_width_rx,
            args.buffer_size,
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
                        match verify_destination(&dest_path, &copy_result.source_hash, &multi, &shutdown, &term_width_rx, args.buffer_size) {
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
                                    // User confirmed deletion
                                    if let Err(e) = fs::remove_file(&dest_path) {
                                        eprintln!("Warning: Failed to remove destination file: {}", e);
                                    } else {
                                        eprintln!("Destination file deleted.");
                                    }
                                    let error_msg = "Verification cancelled by user".to_string();
                                    if args.continue_on_error {
                                        failures.push((source.clone(), error_msg));
                                    } else {
                                        anyhow::bail!("Verification cancelled by user");
                                    }
                                    break VerifyOutcome::Failed;
                                } else {
                                    // User declined deletion - restart verification
                                    eprintln!("Restarting verification...");
                                    continue;
                                }
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

                // Remove source if --rm and verification passed (or was skipped, which is blocked by flag validation)
                let should_allow_removal = matches!(verify_outcome, VerifyOutcome::Passed { .. } | VerifyOutcome::Skipped);
                let removed = if args.rm && should_allow_removal {
                    match fs::remove_file(source) {
                        Ok(()) => true,
                        Err(e) => {
                            let error_msg = format!("Failed to remove source '{}': {}", source.display(), e);
                            eprintln!("\n{}", error_msg);
                            if args.continue_on_error {
                                failures.push((source.clone(), error_msg));
                            } else {
                                anyhow::bail!("{}", error_msg);
                            }
                            false
                        }
                    }
                } else {
                    false
                };

                // Print per-file stats
                let filename = source
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| source.display().to_string());

                let status = match &verify_outcome {
                    VerifyOutcome::Failed => "fail".red(),
                    VerifyOutcome::Passed { .. } | VerifyOutcome::Skipped if args.rm && !removed => {
                        "partial".yellow()
                    }
                    _ => "ok".green(),
                };

                match &verify_outcome {
                    VerifyOutcome::Passed { speed, time, .. } => {
                        println!(
                            "{} {} -> '{}' ({}, copy: {} @ {}, verify: {} @ {})",
                            status,
                            filename,
                            dest_path.display(),
                            HumanBytes(copy_result.bytes_copied),
                            copy_time,
                            copy_speed,
                            time,
                            speed
                        );
                    }
                    VerifyOutcome::Skipped | VerifyOutcome::Failed => {
                        // --no-verify was used, or verification failed (error already printed)
                        println!(
                            "{} {} -> '{}' ({}, {} @ {})",
                            status,
                            filename,
                            dest_path.display(),
                            HumanBytes(copy_result.bytes_copied),
                            copy_time,
                            copy_speed
                        );
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

    // Signal key listener to exit (operation complete)
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

/// A Blake3 hash value.
///
/// This newtype wrapper provides type safety to ensure we don't accidentally
/// confuse hash values with other byte arrays, and provides convenient methods
/// for hash operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Blake3Hash(blake3::Hash);

impl Blake3Hash {
    /// Convert the hash to a hexadecimal string representation.
    fn to_hex(self) -> String {
        self.0.to_hex().to_string()
    }
}

impl From<blake3::Hash> for Blake3Hash {
    fn from(hash: blake3::Hash) -> Self {
        Self(hash)
    }
}

/// Result of a copy operation, including bytes copied, source hash, and timing
struct CopyResult {
    bytes_copied: u64,
    source_hash: Blake3Hash,
    copy_duration: Duration,
}

/// Result of a verification operation, including timing information
struct VerifyResult {
    verify_duration: Duration,
}

/// Error type for verification operations.
///
/// Distinguishes between user cancellation (Ctrl+C) and actual verification
/// failures, allowing different handling for each case.
enum VerifyError {
    /// User cancelled verification with Ctrl+C
    Cancelled,
    /// Verification failed (hash mismatch, I/O error, etc.)
    Failed(String),
}

/// Outcome of the verification step for a file copy.
///
/// This enum makes the verification semantics explicit rather than using
/// a confusing tuple of `(Option<stats>, bool)` where `true` could mean
/// either "verification passed" or "verification was skipped".
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

/// Calculate Blake3 hash of a file with optional progress indicator, cancellation, and resize support.
///
/// If `shutdown` is provided and becomes true during calculation, returns an error
/// indicating cancellation. This allows the caller to handle Ctrl+C gracefully.
///
/// If `term_width_rx` and `filename` are provided, the progress bar style will be
/// updated when the terminal is resized.
fn calculate_file_hash(
    path: &Path,
    pb: Option<&ProgressBar>,
    shutdown: Option<&Arc<AtomicBool>>,
    term_width_rx: Option<&watch::Receiver<u16>>,
    filename: Option<&str>,
    buffer_size: usize,
) -> Result<Blake3Hash> {
    let mut file = File::open(path).context("Failed to open file for hash verification")?;

    // Hint sequential read pattern for better kernel read-ahead
    hint_sequential_io(&file);

    let mut hasher = blake3::Hasher::new();
    let mut buffer = create_uninit_buffer(buffer_size);
    let mut bytes_hashed = 0u64;

    // Track last terminal width for resize detection
    let mut last_width = term_width_rx.map(|rx| *rx.borrow());

    // Throttle UI updates to 5 per second max (every 200ms)
    // Check time every 8 iterations to reduce Instant::now() overhead
    const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
    const TIME_CHECK_INTERVAL: u32 = 8;
    let mut last_update = Instant::now();
    let mut iteration_count: u32 = 0;

    loop {
        // Check for cancellation before each read (keep responsive)
        if let Some(shutdown_flag) = shutdown {
            if shutdown_flag.load(Ordering::SeqCst) {
                anyhow::bail!("Verification cancelled by user");
            }
        }

        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        hasher.update(&buffer[..bytes_read]);
        bytes_hashed += bytes_read as u64;
        iteration_count = iteration_count.wrapping_add(1);

        // Throttle UI updates - only check time every N iterations to reduce overhead
        if iteration_count % TIME_CHECK_INTERVAL == 0 {
            let now = Instant::now();
            if now.duration_since(last_update) >= UPDATE_INTERVAL {
                last_update = now;

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

                if let Some(progress) = pb {
                    progress.set_position(bytes_hashed);
                }
            }
        }
    }

    // Final progress update to ensure we show 100%
    if let Some(progress) = pb {
        progress.set_position(bytes_hashed);
    }

    Ok(Blake3Hash::from(hasher.finalize()))
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
fn format_speed(bytes: u64, duration: Duration) -> String {
    // Use Duration's is_zero() instead of floating-point equality comparison
    if duration.is_zero() {
        return "instant".to_string();
    }
    let bytes_per_sec = bytes as f64 / duration.as_secs_f64();
    // Clamp to u64::MAX to prevent overflow when casting from f64.
    // We also handle negative values (which shouldn't occur) by treating them as 0.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let bytes_per_sec_u64 = if bytes_per_sec <= 0.0 {
        0
    } else if bytes_per_sec >= u64::MAX as f64 {
        u64::MAX
    } else {
        bytes_per_sec as u64
    };
    format!("{}/s", HumanBytes(bytes_per_sec_u64))
}

/// Verify destination matches the expected hash.
///
/// This function performs Blake3 hash verification to ensure the copy was
/// successful. Returns `Ok(VerifyResult)` on success, or `Err(VerifyError)`
/// if verification fails or is cancelled.
///
/// Shows a progress bar for the hash verification process.
/// Supports cancellation via Ctrl+C through the shutdown flag.
/// Supports dynamic resize through the terminal width watch channel.
fn verify_destination(
    destination: &Path,
    expected_hash: &Blake3Hash,
    multi: &MultiProgress,
    shutdown: &Arc<AtomicBool>,
    term_width_rx: &watch::Receiver<u16>,
    buffer_size: usize,
) -> Result<VerifyResult, VerifyError> {
    let start_time = Instant::now();

    // Get file size for progress bar
    let file_size = fs::metadata(destination)
        .map_err(|e| VerifyError::Failed(format!("Failed to get destination metadata: {}", e)))?
        .len();

    // Create progress bar for hash verification
    let filename = destination
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| destination.display().to_string());

    // Set initial style using current terminal width
    let current_width = *term_width_rx.borrow();
    let pb = multi.add(ProgressBar::new(file_size));
    pb.set_style(
        create_verify_style(&filename, current_width)
            .map_err(|e| VerifyError::Failed(format!("Failed to create progress bar: {}", e)))?
            .progress_chars(PROGRESS_CHARS),
    );

    // Calculate destination hash with progress (supports cancellation and resize)
    let dest_hash = calculate_file_hash(destination, Some(&pb), Some(shutdown), Some(term_width_rx), Some(&filename), buffer_size)
        .map_err(|e| {
            pb.finish_and_clear();
            let error_msg = e.to_string();
            if error_msg.contains("cancelled") {
                VerifyError::Cancelled
            } else {
                VerifyError::Failed(format!("Failed to verify destination: {}", e))
            }
        })?;

    pb.finish_and_clear();
    multi.remove(&pb);

    // Verify hashes match
    if *expected_hash != dest_hash {
        return Err(VerifyError::Failed(format!(
            "Hash mismatch!\n  Expected: {}\n  Got:      {}",
            expected_hash.to_hex(),
            dest_hash.to_hex()
        )));
    }

    let verify_duration = start_time.elapsed();
    Ok(VerifyResult { verify_duration })
}

/// Prompt user to confirm copy cancellation.
///
/// Returns true if user confirms cancellation, false to continue.
/// Uses crossterm event reading to capture Ctrl+C as a key event (not SIGINT).
/// Pressing Ctrl+C at this prompt is treated as confirmation to cancel.
fn prompt_cancel_copy(destination: &Path, input_active: &Arc<AtomicBool>) -> bool {
    // Pause key listener while we handle input ourselves
    input_active.store(true, Ordering::SeqCst);

    // Disable raw mode temporarily to print prompt with proper line handling
    let _ = crossterm::terminal::disable_raw_mode();
    eprint!(
        "\nCancel copy? Partial file '{}' will be deleted. (y/N): ",
        destination.display()
    );
    let _ = io::stderr().flush();

    // Re-enable raw mode to capture Ctrl+C as key event (not SIGINT)
    let _ = crossterm::terminal::enable_raw_mode();

    // Read user response using crossterm events
    let confirmed = read_yes_no_with_ctrlc();

    // Resume key listener
    input_active.store(false, Ordering::SeqCst);

    confirmed
}

/// Read a yes/no response using crossterm events.
/// Returns true for 'y'/'Y' or Ctrl+C, false for any other input followed by Enter.
fn read_yes_no_with_ctrlc() -> bool {
    let mut input = String::new();

    loop {
        if event::poll(Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(Event::Key(key_event)) = event::read() {
                // Only process key press events, not release events
                if key_event.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }

                match key_event.code {
                    KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                        // Ctrl+C means "yes, cancel"
                        // Print newline for clean output
                        let _ = crossterm::terminal::disable_raw_mode();
                        eprintln!();
                        let _ = crossterm::terminal::enable_raw_mode();
                        return true;
                    }
                    KeyCode::Enter => {
                        // Print newline for clean output
                        let _ = crossterm::terminal::disable_raw_mode();
                        eprintln!();
                        let _ = crossterm::terminal::enable_raw_mode();
                        // Check if input was "y" or "Y"
                        return input.trim().eq_ignore_ascii_case("y");
                    }
                    KeyCode::Char(c) => {
                        input.push(c);
                        // Echo the character
                        let _ = crossterm::terminal::disable_raw_mode();
                        eprint!("{}", c);
                        let _ = io::stderr().flush();
                        let _ = crossterm::terminal::enable_raw_mode();
                    }
                    KeyCode::Backspace => {
                        if input.pop().is_some() {
                            // Erase character from display
                            let _ = crossterm::terminal::disable_raw_mode();
                            eprint!("\x08 \x08"); // backspace, space, backspace
                            let _ = io::stderr().flush();
                            let _ = crossterm::terminal::enable_raw_mode();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Attempt to preallocate space for the destination file.
///
/// This helps the filesystem allocate contiguous blocks, reducing fragmentation
/// and potentially improving write performance. Errors are silently ignored
/// since preallocation is an optimization, not a requirement.
#[cfg(target_os = "linux")]
fn try_preallocate(file: &File, size: u64) {
    use std::os::unix::io::AsRawFd;
    // posix_fallocate returns 0 on success, error code on failure
    // We ignore the result since this is just an optimization
    #[allow(clippy::cast_possible_wrap)] // size as libc::off_t is safe for typical file sizes
    unsafe {
        libc::posix_fallocate(file.as_raw_fd(), 0, size as libc::off_t);
    }
}

/// Attempt to preallocate space on macOS using F_PREALLOCATE.
#[cfg(target_os = "macos")]
fn try_preallocate(file: &File, size: u64) {
    use std::os::unix::io::AsRawFd;

    // macOS uses fcntl with F_PREALLOCATE
    #[repr(C)]
    struct FStore {
        fst_flags: libc::c_uint,
        fst_posmode: libc::c_int,
        fst_offset: libc::off_t,
        fst_length: libc::off_t,
        fst_bytesalloc: libc::off_t,
    }

    const F_ALLOCATECONTIG: libc::c_uint = 0x02; // Allocate contiguous space
    const F_ALLOCATEALL: libc::c_uint = 0x04; // Allocate all requested space or none
    const F_PEOFPOSMODE: libc::c_int = 3; // Position relative to physical end of file
    const F_PREALLOCATE: libc::c_int = 42; // Preallocate storage

    #[allow(clippy::cast_possible_wrap)]
    let mut fstore = FStore {
        fst_flags: F_ALLOCATECONTIG | F_ALLOCATEALL,
        fst_posmode: F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: size as libc::off_t,
        fst_bytesalloc: 0,
    };

    unsafe {
        // Try contiguous allocation first, fall back to any allocation
        if libc::fcntl(file.as_raw_fd(), F_PREALLOCATE, &mut fstore) == -1 {
            // If contiguous allocation fails, try non-contiguous
            fstore.fst_flags = F_ALLOCATEALL;
            let _ = libc::fcntl(file.as_raw_fd(), F_PREALLOCATE, &mut fstore);
        }
    }
}

/// No-op on platforms without preallocation support.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn try_preallocate(_file: &File, _size: u64) {}

/// Hint to the OS that we're doing sequential I/O.
///
/// This allows the kernel to optimize read-ahead and caching strategies
/// for sequential access patterns. Errors are silently ignored since
/// this is just a performance hint.
#[cfg(target_os = "linux")]
fn hint_sequential_io(file: &File) {
    use std::os::unix::io::AsRawFd;
    // POSIX_FADV_SEQUENTIAL hints that file will be accessed sequentially
    // We ignore the result since this is just a hint
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
    }
}

/// Hint for sequential I/O on macOS using F_RDAHEAD.
#[cfg(target_os = "macos")]
fn hint_sequential_io(file: &File) {
    use std::os::unix::io::AsRawFd;
    const F_RDAHEAD: libc::c_int = 45; // Turn read-ahead on/off
    unsafe {
        // Enable read-ahead (1 = on)
        let _ = libc::fcntl(file.as_raw_fd(), F_RDAHEAD, 1);
    }
}

/// No-op on platforms without sequential I/O hints.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn hint_sequential_io(_file: &File) {}

#[allow(clippy::too_many_arguments)] // All parameters serve distinct purposes for progress/cancellation/resize
async fn copy_with_progress(
    source: &Path,
    destination: &Path,
    pb: &ProgressBar,
    filename: &str,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    input_active: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
    term_width_rx: &watch::Receiver<u16>,
    buffer_size: usize,
) -> Result<CopyResult> {
    let start_time = Instant::now();
    let mut src_file = File::open(source).context("Failed to open source file")?;

    // Get source file size for preallocation
    let file_size = src_file
        .metadata()
        .context("Failed to get source file metadata")?
        .len();

    // Hint sequential read pattern for source file
    hint_sequential_io(&src_file);

    let dst_file = File::create(destination).context("Failed to create destination file")?;

    // Hint sequential write pattern for destination file
    hint_sequential_io(&dst_file);

    // Preallocate space to reduce fragmentation and improve write performance
    try_preallocate(&dst_file, file_size);

    // Use RAII guard to ensure partial file cleanup on any error path
    // (Ctrl+C, I/O errors, etc.). The guard is defused on successful completion.
    let mut guard = PartialFileGuard::new(destination, dst_file);

    let mut buffer = create_uninit_buffer(buffer_size);
    let mut total_bytes = 0u64;
    let mut hasher = blake3::Hasher::new();

    // Track last terminal width for resize detection
    let mut last_width = *term_width_rx.borrow();

    // Throttle UI updates to 5 per second max (every 200ms)
    // Check time every 8 iterations to reduce Instant::now() overhead
    const UPDATE_INTERVAL: Duration = Duration::from_millis(200);
    const TIME_CHECK_INTERVAL: u32 = 8;
    let mut last_update = Instant::now();
    let mut iteration_count: u32 = 0;

    loop {
        // Check for shutdown - prompt user for confirmation (keep responsive)
        if shutdown.load(Ordering::SeqCst) {
            if prompt_cancel_copy(destination, &input_active) {
                return Err(anyhow::anyhow!(
                    "Copy cancelled by user (partial destination file deleted)"
                ));
            }
            // User declined cancellation - reset flag and continue
            shutdown.store(false, Ordering::SeqCst);
        }

        // Check for pause toggle (keep responsive)
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
            // Check for terminal resize while paused
            let current_width = *term_width_rx.borrow();
            if current_width != last_width {
                last_width = current_width;
                if let Ok(style) = create_copy_style(filename, current_width) {
                    pb.set_style(style.progress_chars(PROGRESS_CHARS));
                }
            }

            // Check for shutdown while paused - prompt user for confirmation
            if shutdown.load(Ordering::SeqCst) {
                if prompt_cancel_copy(destination, &input_active) {
                    return Err(anyhow::anyhow!(
                        "Copy cancelled by user (partial destination file deleted)"
                    ));
                }
                // User declined cancellation - reset flag and continue
                shutdown.store(false, Ordering::SeqCst);
            }

            tokio::time::sleep(Duration::from_millis(100)).await;

            // Check for unpause
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

        // Update hash with the data we just read
        hasher.update(&buffer[..bytes_read]);

        // Write to destination
        guard
            .file_mut()
            .write_all(&buffer[..bytes_read])
            .context("Failed to write to destination file")?;

        total_bytes += bytes_read as u64;
        iteration_count = iteration_count.wrapping_add(1);

        // Throttle UI updates - only check time every N iterations to reduce overhead
        if iteration_count % TIME_CHECK_INTERVAL == 0 {
            let now = Instant::now();
            if now.duration_since(last_update) >= UPDATE_INTERVAL {
                last_update = now;

                // Check for terminal resize and update progress bar style
                let current_width = *term_width_rx.borrow();
                if current_width != last_width {
                    last_width = current_width;
                    if let Ok(style) = create_copy_style(filename, current_width) {
                        pb.set_style(style.progress_chars(PROGRESS_CHARS));
                    }
                }

                pb.set_position(total_bytes);
            }
        }
    }

    // Final progress update to ensure we show 100%
    pb.set_position(total_bytes);

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

    // Explicitly close the destination file before verification can occur
    drop(dst_file);

    // Finalize the hash
    let source_hash = Blake3Hash::from(hasher.finalize());
    let copy_duration = start_time.elapsed();

    Ok(CopyResult {
        bytes_copied: total_bytes,
        source_hash,
        copy_duration,
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
        fn zero_duration_returns_instant() {
            assert_eq!(format_speed(1000, Duration::ZERO), "instant");
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
            assert!(result.ends_with("/s") || result == "instant");
        }

        #[test]
        fn very_large_bytes() {
            // Test with u64::MAX bytes
            let result = format_speed(u64::MAX, Duration::from_secs(1));
            assert!(result.ends_with("/s"));
        }
    }

    mod parse_buffer_size_tests {
        use super::*;

        #[test]
        fn parse_bytes_no_suffix() {
            assert_eq!(parse_buffer_size("65536").unwrap(), 65536);
        }

        #[test]
        fn parse_kilobytes() {
            assert_eq!(parse_buffer_size("4K").unwrap(), 4 * 1024);
            assert_eq!(parse_buffer_size("4KB").unwrap(), 4 * 1024);
            assert_eq!(parse_buffer_size("4k").unwrap(), 4 * 1024);
        }

        #[test]
        fn parse_megabytes() {
            assert_eq!(parse_buffer_size("16M").unwrap(), 16 * 1024 * 1024);
            assert_eq!(parse_buffer_size("16MB").unwrap(), 16 * 1024 * 1024);
            assert_eq!(parse_buffer_size("16m").unwrap(), 16 * 1024 * 1024);
        }

        #[test]
        fn parse_gigabytes() {
            assert_eq!(parse_buffer_size("1G").unwrap(), 1024 * 1024 * 1024);
            assert_eq!(parse_buffer_size("1GB").unwrap(), 1024 * 1024 * 1024);
        }

        #[test]
        fn parse_with_whitespace() {
            assert_eq!(parse_buffer_size("  16M  ").unwrap(), 16 * 1024 * 1024);
        }

        #[test]
        fn reject_below_minimum() {
            let result = parse_buffer_size("1K");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("below minimum"));
        }

        #[test]
        fn reject_above_maximum() {
            let result = parse_buffer_size("2G");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("exceeds maximum"));
        }

        #[test]
        fn reject_invalid_suffix() {
            let result = parse_buffer_size("16X");
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("Unknown suffix"));
        }

        #[test]
        fn reject_invalid_number() {
            let result = parse_buffer_size("abc");
            assert!(result.is_err());
        }

        #[test]
        fn accept_boundary_minimum() {
            // Exactly 4KB should be accepted
            assert_eq!(parse_buffer_size("4K").unwrap(), 4 * 1024);
        }

        #[test]
        fn accept_boundary_maximum() {
            // Exactly 1GB should be accepted
            assert_eq!(parse_buffer_size("1G").unwrap(), 1024 * 1024 * 1024);
        }
    }
}
