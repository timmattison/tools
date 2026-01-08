use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task;

#[derive(Parser, Debug)]
#[command(author, version, about = "Progress copy - copy files with progress bar", long_about = None)]
struct Args {
    /// Source files or glob patterns to copy
    #[arg(required = true, num_args = 1..)]
    sources: Vec<PathBuf>,

    /// Destination file or directory path
    destination: PathBuf,

    /// Remove source files after successful copy (verified by SHA256 hash)
    #[arg(long, short = 'r')]
    rm: bool,

    /// Continue copying remaining files if one fails
    #[arg(long)]
    continue_on_error: bool,
}

const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer

/// Resolve source patterns into a list of files
///
/// Handles both literal file paths and glob patterns (*, ?, []).
/// Returns an error if a glob pattern matches no files or a literal path doesn't exist.
fn resolve_sources(patterns: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    for pattern in patterns {
        let pattern_str = pattern.to_string_lossy();

        // Check if pattern contains glob characters
        if pattern_str.contains('*') || pattern_str.contains('?') || pattern_str.contains('[') {
            let matches: Vec<_> = glob::glob(&pattern_str)
                .with_context(|| format!("Invalid glob pattern '{}'", pattern_str))?
                .filter_map(|r| r.ok())
                .filter(|p| p.is_file())
                .collect();

            if matches.is_empty() {
                anyhow::bail!("No files match pattern '{}'", pattern_str);
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

    if files.is_empty() {
        anyhow::bail!("No source files specified");
    }

    Ok(files)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Resolve all source files (handles glob patterns)
    let sources = resolve_sources(&args.sources)?;
    let total_files = sources.len();

    // Validate destination for multi-file operations
    if total_files > 1 {
        // For multiple files, destination must be a directory
        // Check if exists and is NOT a directory (error case)
        if args.destination.exists() && !args.destination.is_dir() {
            anyhow::bail!(
                "Destination '{}' is not a directory (required for multiple source files)",
                args.destination.display()
            );
        }
        // Create destination directory if needed (idempotent if already exists as dir)
        fs::create_dir_all(&args.destination)
            .context("Failed to create destination directory")?;
    }

    // Warn about potentially dangerous combination
    if args.rm && args.continue_on_error && total_files > 1 {
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

    // Enable raw mode for keyboard input
    let raw_mode_enabled = crossterm::terminal::enable_raw_mode().is_ok();

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
        let destination = if args.destination.is_dir() {
            let filename = source
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("Source '{}' has no filename", source.display()))?;
            args.destination.join(filename)
        } else {
            args.destination.clone()
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
        if destination.exists() {
            eprintln!(
                "\nDestination '{}' already exists. Overwrite? (y/N): ",
                destination.display()
            );
            io::stderr().flush()?;

            // Temporarily disable raw mode for input
            if raw_mode_enabled {
                let _ = crossterm::terminal::disable_raw_mode();
            }

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            // Re-enable raw mode
            if raw_mode_enabled {
                let _ = crossterm::terminal::enable_raw_mode();
            }

            if !input.trim().eq_ignore_ascii_case("y") {
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
        if let Some(parent) = destination.parent() {
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
                        destination.display(),
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

        file_pb.set_style(
            ProgressStyle::default_bar()
                .template(&format!(
                    "{{spinner:.green}} {} [{{bar:40.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}}, {{eta}})",
                    filename
                ))?
                .progress_chars("█▉▊▋▌▍▎▏  "),
        );

        // Perform the copy
        let result = copy_with_progress(
            source,
            &destination,
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

                // Handle --rm flag
                if args.rm {
                    let dest_hash = match calculate_file_hash(&destination) {
                        Ok(h) => h,
                        Err(e) => {
                            let error_msg =
                                format!("Failed to verify destination: {}. Source NOT removed.", e);
                            eprintln!("\n{}", error_msg);
                            if args.continue_on_error {
                                failures.push((source.clone(), error_msg));
                                if let Some(ref pb) = overall_pb {
                                    pb.inc(1);
                                }
                                continue;
                            } else {
                                anyhow::bail!("{}", error_msg);
                            }
                        }
                    };

                    if copy_result.source_hash == dest_hash {
                        if let Err(e) = fs::remove_file(source) {
                            let error_msg = format!("Failed to remove source: {}", e);
                            if args.continue_on_error {
                                failures.push((source.clone(), error_msg));
                            } else {
                                anyhow::bail!(
                                    "Failed to remove source file '{}': {}",
                                    source.display(),
                                    e
                                );
                            }
                        }
                    } else {
                        let error_msg = format!(
                            "Hash mismatch! Source NOT removed.\n  Source: {}\n  Dest:   {}",
                            hex_encode(&copy_result.source_hash),
                            hex_encode(&dest_hash)
                        );
                        if args.continue_on_error {
                            eprintln!("\n{}", error_msg);
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
                        destination.display()
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
                    // Clean up and bail
                    shutdown.store(true, Ordering::SeqCst);
                    if raw_mode_enabled {
                        let _ = crossterm::terminal::disable_raw_mode();
                    }
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

    // Disable raw mode
    if raw_mode_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }

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
    source_hash: [u8; 32],
}

/// Calculate SHA256 hash of a file by reading it completely
fn calculate_file_hash(path: &Path) -> Result<[u8; 32]> {
    let mut file = File::open(path).context("Failed to open file for hash verification")?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER_SIZE];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize().into())
}

/// Convert a byte array to a hex string
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

async fn copy_with_progress(
    source: &Path,
    destination: &Path,
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<CopyResult> {
    let mut src_file = File::open(source).context("Failed to open source file")?;
    let mut dst_file = File::create(destination).context("Failed to create destination file")?;

    let mut buffer = vec![0; BUFFER_SIZE];
    let mut total_bytes = 0u64;
    let mut hasher = Sha256::new();

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
            // Check for shutdown while paused
            if shutdown.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Copy cancelled by user"));
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
        dst_file
            .write_all(&buffer[..bytes_read])
            .context("Failed to write to destination file")?;

        total_bytes += bytes_read as u64;
        pb.set_position(total_bytes);
    }

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
    let source_hash: [u8; 32] = hasher.finalize().into();

    Ok(CopyResult {
        bytes_copied: total_bytes,
        source_hash,
    })
}