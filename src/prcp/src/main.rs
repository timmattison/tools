// Clippy lints to catch common issues
#![warn(clippy::unwrap_used)] // Prefer explicit error handling
#![warn(clippy::expect_used)] // Prefer explicit error handling
#![warn(clippy::panic)] // Avoid panics in library code
#![deny(clippy::unimplemented)] // Don't leave unimplemented!() in code

use anyhow::{Context, Result};
use clap::Parser;
use colored::Colorize;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(target_os = "macos")]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task;

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

#[derive(Parser, Debug)]
#[command(author, version, about = "Progress copy - copy files with progress bar", long_about = None)]
struct Args {
    /// Source file(s) and destination (last argument is destination)
    #[arg(num_args = 0..)]
    paths: Vec<PathBuf>,

    /// Add shell integration (prmv function) to your shell config
    #[arg(long)]
    shell_setup: bool,

    /// Remove source files after successful copy (verified by SHA256 hash)
    #[arg(long, short = 'r')]
    rm: bool,

    /// Continue copying remaining files if one fails
    #[arg(long)]
    continue_on_error: bool,

    /// Skip all confirmation prompts (assume yes)
    #[arg(long, short = 'y')]
    yes: bool,
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

const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer

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

    // Set up shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));

    // Set up pause/resume handling
    let paused = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let shutdown_key_listener = shutdown.clone();

    // Spawn key listener task
    let key_task = task::spawn(async move {
        loop {
            if shutdown_key_listener.load(Ordering::SeqCst) {
                break;
            }

            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                if let Ok(Event::Key(key_event)) = event::read() {
                    match key_event.code {
                        KeyCode::Char(' ') => {
                            let _ = tx.send(());
                        }
                        KeyCode::Char('c')
                            if key_event.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            shutdown_key_listener.store(true, Ordering::SeqCst);
                            break;
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Enable raw mode for keyboard input (uses RAII guard for safety)
    let mut raw_mode_guard = RawModeGuard::new();

    // Set up multi-progress display
    let multi = MultiProgress::new();

    // Overall progress bar (only shown for multiple files)
    let overall_pb = if total_files > 1 {
        let pb = multi.add(ProgressBar::new(total_files as u64));
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{prefix:.bold} [{bar:40.green/dim}] {pos}/{len}")?
                .progress_chars("█▉▊▋▌▍▎▏  "),
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

    // Copy each file
    for source in &sources {
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
                eprintln!(
                    "\nDestination '{}' already exists. Overwrite? (y/N): ",
                    dest_path.display()
                );
                io::stderr().flush()?;

                // Temporarily disable raw mode for input (guard ensures restoration)
                raw_mode_guard.disable_temporarily();

                let mut input = String::new();
                io::stdin().read_line(&mut input)?;

                // Re-enable raw mode
                raw_mode_guard.restore();

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

        // Escape braces in filename to prevent template injection
        // (indicatif templates use {placeholder} syntax)
        let safe_filename = escape_template_braces(&filename);

        file_pb.set_style(
            ProgressStyle::default_bar()
                .template(&format!(
                    "{{spinner:.green}} {} [{{bar:40.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}}, {{eta}})",
                    safe_filename
                ))?
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );

        // Perform the copy
        let result = copy_with_progress(
            source,
            &dest_path,
            &file_pb,
            paused.clone(),
            shutdown.clone(),
            &mut rx,
        )
        .await;

        file_pb.finish_and_clear();
        multi.remove(&file_pb);

        match result {
            Ok(copy_result) => {
                successful_copies += 1;
                total_bytes_copied += copy_result.bytes_copied;

                // Handle --rm flag: verify copy and remove source
                if args.rm {
                    if let Err(error_msg) =
                        verify_and_remove_source(source, &dest_path, &copy_result.source_hash)
                    {
                        eprintln!("\n{}", error_msg);
                        if args.continue_on_error {
                            failures.push((source.clone(), error_msg));
                        } else {
                            anyhow::bail!("{}", error_msg);
                        }
                    }
                }

                if total_files == 1 {
                    println!(
                        "\nSuccessfully copied {} to '{}'",
                        HumanBytes(copy_result.bytes_copied),
                        dest_path.display()
                    );
                    if args.rm {
                        println!("Source file removed after verification.");
                    }
                }
            }
            Err(e) => {
                let error_msg = format!("{}", e);
                if args.continue_on_error {
                    failures.push((source.clone(), error_msg));
                } else {
                    // Clean up and bail (raw mode cleaned up by RawModeGuard on drop)
                    shutdown.store(true, Ordering::SeqCst);
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

    // Signal shutdown to stop the key listener
    shutdown.store(true, Ordering::SeqCst);

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
        println!(
            "\nCopied {} of {} files ({} total)",
            successful_copies,
            total_files,
            HumanBytes(total_bytes_copied)
        );
        if args.rm && successful_copies > 0 {
            println!("Source files removed after verification.");
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

/// Result of a copy operation, including bytes copied and source hash
struct CopyResult {
    bytes_copied: u64,
    source_hash: blake3::Hash,
}

/// Disable page cache for a file on macOS using F_NOCACHE.
/// This improves throughput for large sequential reads/writes by avoiding
/// unnecessary memory copies through the kernel page cache.
#[cfg(target_os = "macos")]
fn disable_page_cache(file: &File) -> Result<()> {
    // F_NOCACHE = 48 on macOS
    const F_NOCACHE: libc::c_int = 48;
    let fd = file.as_raw_fd();
    let result = unsafe { libc::fcntl(fd, F_NOCACHE, 1) };
    if result == -1 {
        anyhow::bail!("Failed to set F_NOCACHE: {}", io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn disable_page_cache(_file: &File) -> Result<()> {
    // No-op on non-macOS platforms
    Ok(())
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

/// Calculate Blake3 hash of a file by reading it completely.
/// Uses F_NOCACHE on macOS for better throughput on large files.
fn calculate_file_hash(path: &Path) -> Result<blake3::Hash> {
    let file = File::open(path).context("Failed to open file for hash verification")?;
    // Ignore F_NOCACHE errors - it's an optimization, not required
    let _ = disable_page_cache(&file);

    let mut reader = io::BufReader::with_capacity(BUFFER_SIZE, file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];

    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize())
}

/// Escape braces in a string for use in indicatif templates.
/// Indicatif uses `{placeholder}` syntax, so literal braces must be doubled.
fn escape_template_braces(s: &str) -> String {
    s.replace('{', "{{").replace('}', "}}")
}

/// Verify destination matches source and remove the source file.
///
/// This function performs Blake3 hash verification to ensure the copy was
/// successful before removing the source. Returns `Ok(())` on success,
/// or `Err(error_message)` if verification or removal fails.
fn verify_and_remove_source(
    source: &Path,
    destination: &Path,
    expected_hash: &blake3::Hash,
) -> Result<(), String> {
    // Calculate destination hash
    let dest_hash = calculate_file_hash(destination)
        .map_err(|e| format!("Failed to verify destination: {}. Source NOT removed.", e))?;

    // Verify hashes match
    if expected_hash != &dest_hash {
        return Err(format!(
            "Hash mismatch! Source NOT removed.\n  Source: {}\n  Dest:   {}",
            expected_hash.to_hex(),
            dest_hash.to_hex()
        ));
    }

    // Safe to remove source
    fs::remove_file(source)
        .map_err(|e| format!("Failed to remove source '{}': {}", source.display(), e))?;

    Ok(())
}

/// Message sent from reader thread to writer thread (used in parallel copy mode)
enum CopyMessage {
    /// Data chunk to write (buffer contents, length)
    Data(Vec<u8>),
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
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    if same_device(source, destination) {
        copy_sequential(source, destination, pb, paused, shutdown, rx).await
    } else {
        copy_parallel(source, destination, pb, paused, shutdown, rx).await
    }
}

/// Sequential copy for same-device scenarios (optimal for spinning HDDs).
/// Performs read→hash→write in sequence to avoid disk head thrashing.
async fn copy_sequential(
    source: &Path,
    destination: &Path,
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    let mut src_file = File::open(source).context("Failed to open source file")?;
    let mut dst_file = File::create(destination).context("Failed to create destination file")?;
    let _ = disable_page_cache(&src_file);
    let _ = disable_page_cache(&dst_file);

    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut total_bytes = 0u64;
    let mut hasher = blake3::Hasher::new();

    loop {
        // Check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Copy cancelled by user"));
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
            if shutdown.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Copy cancelled by user"));
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

        // Hash and write (sequential - no head thrashing on same device)
        hasher.update(&buffer[..bytes_read]);
        dst_file
            .write_all(&buffer[..bytes_read])
            .context("Failed to write to destination file")?;

        total_bytes += bytes_read as u64;
        pb.set_position(total_bytes);
    }

    // Finalize
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
    })
}

/// Parallel copy for cross-device scenarios (optimal for SSDs/NVMe).
/// Uses dedicated reader/writer threads for true parallelism.
/// - Reader thread: continuously reads into buffers, sends to channel
/// - Writer task: receives from channel, hashes, writes to disk
/// - Main async task: handles pause/resume/shutdown coordination
async fn copy_parallel(
    source: &Path,
    destination: &Path,
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    let src_path = source.to_path_buf();
    let dst_path = destination.to_path_buf();

    // Open files with F_NOCACHE
    let src_file = File::open(&src_path).context("Failed to open source file")?;
    let dst_file = File::create(&dst_path).context("Failed to create destination file")?;
    let _ = disable_page_cache(&src_file);
    let _ = disable_page_cache(&dst_file);

    // Channel for passing data from reader to writer (bounded to limit memory)
    let (data_tx, data_rx) = std::sync::mpsc::sync_channel::<CopyMessage>(2);

    // Shared state for coordination
    let reader_paused = Arc::clone(&paused);
    let reader_shutdown = Arc::clone(&shutdown);

    // Spawn reader thread
    let reader_handle = std::thread::spawn(move || {
        let mut file = src_file;
        let mut buffer = vec![0u8; BUFFER_SIZE];

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

            // Read chunk
            match file.read(&mut buffer) {
                Ok(0) => {
                    let _ = data_tx.send(CopyMessage::Eof);
                    return;
                }
                Ok(n) => {
                    // Send copy of data (only the bytes that were read)
                    let data = buffer[..n].to_vec();
                    if data_tx.send(CopyMessage::Data(data)).is_err() {
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
    let mut file = dst_file;
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

        // Non-blocking check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            // Wait for reader to notice and exit
            let _ = reader_handle.join();
            return Err(anyhow::anyhow!("Copy cancelled by user"));
        }

        // Wait while paused (but keep checking for shutdown/unpause)
        while paused.load(Ordering::SeqCst) {
            if shutdown.load(Ordering::SeqCst) {
                let _ = reader_handle.join();
                return Err(anyhow::anyhow!("Copy cancelled by user"));
            }

            if rx.try_recv().is_ok() {
                paused.store(false, Ordering::SeqCst);
                pb.set_message("");
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Try to receive data with timeout to allow checking pause/shutdown
        match data_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(CopyMessage::Data(data)) => {
                // Hash and write
                hasher.update(&data);
                file.write_all(&data)
                    .context("Failed to write to destination file")?;
                total_bytes += data.len() as u64;
                pb.set_position(total_bytes);
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

    // Finalize
    file.flush().context("Failed to flush destination file")?;
    file.sync_all()
        .context("Failed to sync destination file to disk")?;

    // Copy file permissions
    let metadata = fs::metadata(&src_path)?;
    fs::set_permissions(&dst_path, metadata.permissions())?;

    // Explicitly close the destination file
    drop(file);

    Ok(CopyResult {
        bytes_copied: total_bytes,
        source_hash: hasher.finalize(),
    })
}