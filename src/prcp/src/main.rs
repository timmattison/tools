use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};
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
    
    /// Destination file path
    destination: PathBuf,
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
    
    // Get file metadata
    let metadata = fs::metadata(&args.source)
        .context("Failed to read source file metadata")?;
    let total_size = metadata.len();
    
    // Check if destination exists
    if args.destination.exists() {
        eprintln!("Destination '{}' already exists", args.destination.display());
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
    if let Some(parent) = args.destination.parent() {
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
        &args.destination,
        &pb,
        paused,
        shutdown.clone(),
        &mut rx,
    ).await;
    
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
        Ok(bytes_copied) => {
            println!("\nSuccessfully copied {} bytes", HumanBytes(bytes_copied));
            Ok(())
        }
        Err(e) => {
            eprintln!("\nError during copy: {}", e);
            Err(e)
        }
    }
}

async fn copy_with_progress(
    source: &PathBuf,
    destination: &PathBuf,
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<u64> {
    let mut src_file = File::open(source)
        .context("Failed to open source file")?;
    let mut dst_file = File::create(destination)
        .context("Failed to create destination file")?;
    
    let mut buffer = vec![0; BUFFER_SIZE];
    let mut total_bytes = 0u64;
    
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
        
        // Write to destination
        dst_file.write_all(&buffer[..bytes_read])
            .context("Failed to write to destination file")?;
        
        total_bytes += bytes_read as u64;
        pb.set_position(total_bytes);
    }
    
    // Ensure all data is written
    dst_file.flush()
        .context("Failed to flush destination file")?;
    
    // Copy file permissions
    let metadata = fs::metadata(source)?;
    fs::set_permissions(destination, metadata.permissions())?;
    
    Ok(total_bytes)
}