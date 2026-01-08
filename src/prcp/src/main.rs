use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task;

#[derive(Parser, Debug)]
#[command(author, version, about = "Progress copy - copy files with progress bar", long_about = None)]
struct Args {
    /// Source file to copy
    source: PathBuf,

    /// Destination file or directory path
    destination: PathBuf,

    /// Remove source file after successful copy (verified by SHA256 hash)
    #[arg(long, short = 'r')]
    rm: bool,
}

const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer

#[tokio::main]
async fn main() -> Result<()> {
    // Set up shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));
    
    let args = Args::parse();
    
    // Validate source file exists
    if !args.source.exists() {
        anyhow::bail!("Source file '{}' does not exist", args.source.display());
    }
    
    if !args.source.is_file() {
        anyhow::bail!("Source '{}' is not a file", args.source.display());
    }

    // Resolve destination - if it's a directory, append the source filename
    let destination = if args.destination.is_dir() {
        let filename = args
            .source
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("Source has no filename"))?;
        args.destination.join(filename)
    } else {
        args.destination
    };

    // Get file metadata
    let metadata = fs::metadata(&args.source)
        .context("Failed to read source file metadata")?;
    let total_size = metadata.len();
    
    // Check if destination exists
    if destination.exists() {
        eprintln!("Destination '{}' already exists", destination.display());
        eprint!("Overwrite? (y/N): ");
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Copy cancelled");
            return Ok(());
        }
    }

    // Create parent directories if needed
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .context("Failed to create destination directory")?;
    }
    
    // Set up progress bar
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")?
            .progress_chars("█▉▊▋▌▍▎▏  ")
    );
    
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
                        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
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
    
    // Perform the copy
    let result = copy_with_progress(
        &args.source,
        &destination,
        &pb,
        paused,
        shutdown.clone(),
        &mut rx,
    )
    .await;
    
    // Signal shutdown to stop the key listener
    shutdown.store(true, Ordering::SeqCst);
    
    // Disable raw mode
    if raw_mode_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    
    // Finish progress bar
    pb.finish();
    
    // Wait for key task to finish
    let _ = key_task.await;
    
    match result {
        Ok(copy_result) => {
            println!(
                "\nSuccessfully copied {}",
                HumanBytes(copy_result.bytes_copied)
            );

            // If --rm flag is set, verify destination and delete source
            if args.rm {
                println!("Verifying destination file...");

                // Calculate destination hash by reopening and reading the file
                let dest_hash = calculate_file_hash(&destination)?;

                if copy_result.source_hash == dest_hash {
                    // Hashes match - safe to delete source
                    fs::remove_file(&args.source).context("Failed to remove source file")?;
                    println!(
                        "Verification successful. Source file '{}' removed.",
                        args.source.display()
                    );
                } else {
                    // Hashes don't match - report error
                    let source_hex = hex_encode(&copy_result.source_hash);
                    let dest_hex = hex_encode(&dest_hash);
                    anyhow::bail!(
                        "Hash mismatch! Source file NOT removed for safety.\n  \
                        Source SHA256: {}\n  Dest SHA256:   {}\n  \
                        The copy may have been corrupted. Please verify the destination file and retry.",
                        source_hex,
                        dest_hex
                    );
                }
            }

            Ok(())
        }
        Err(e) => {
            eprintln!("\nError during copy: {}", e);
            Err(e)
        }
    }
}

/// Result of a copy operation, including bytes copied and source hash
struct CopyResult {
    bytes_copied: u64,
    source_hash: [u8; 32],
}

/// Calculate SHA256 hash of a file by reading it completely
fn calculate_file_hash(path: &PathBuf) -> Result<[u8; 32]> {
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
    source: &PathBuf,
    destination: &PathBuf,
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