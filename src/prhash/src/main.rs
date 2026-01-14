use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task;

// Hash algorithm imports
use blake3::Hasher as Blake3Hasher;
use md5::{Md5, Digest};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum HashAlgorithm {
    #[value(name = "md5")]
    Md5,
    #[value(name = "sha1")]
    Sha1,
    #[value(name = "sha256")]
    Sha256,
    #[value(name = "sha512")]
    Sha512,
    #[value(name = "blake3")]
    Blake3,
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Progress hash - compute file hashes with progress bar", long_about = None)]
struct Args {
    /// Hash algorithm to use (defaults to blake3)
    #[arg(short = 'a', long, value_enum)]
    algorithm: Option<HashAlgorithm>,
    
    /// Files to hash
    files: Vec<PathBuf>,
}

const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer
const MAX_FILENAME_DISPLAY_LEN: usize = 60; // Max characters for filename in progress message

/// Truncates a path for display, keeping the filename visible
fn truncate_path_for_display(path: &std::path::Path, max_len: usize) -> String {
    let display = path.display().to_string();
    if display.len() <= max_len {
        return display;
    }

    // Try to keep the filename visible by truncating from the middle
    if let Some(filename) = path.file_name() {
        let filename_str = filename.to_string_lossy();
        if filename_str.len() < max_len - 4 {
            // We have room for filename + ellipsis + some parent path
            let remaining = max_len - filename_str.len() - 4; // 4 for ".../""
            let parent = path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            if !parent.is_empty() && remaining > 0 {
                let truncated_parent: String = parent.chars().take(remaining).collect();
                return format!("{}.../{}", truncated_parent, filename_str);
            }
        }
        // Filename itself is too long, just truncate from start
        return format!("...{}", &display[display.len().saturating_sub(max_len - 3)..]);
    }

    // Fallback: simple truncation from the end
    format!("{}...", &display[..max_len - 3])
}

enum HashState {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
    Blake3(Blake3Hasher),
}

impl HashState {
    fn new(algorithm: HashAlgorithm) -> Self {
        match algorithm {
            HashAlgorithm::Md5 => HashState::Md5(Md5::new()),
            HashAlgorithm::Sha1 => HashState::Sha1(Sha1::new()),
            HashAlgorithm::Sha256 => HashState::Sha256(Sha256::new()),
            HashAlgorithm::Sha512 => HashState::Sha512(Sha512::new()),
            HashAlgorithm::Blake3 => HashState::Blake3(Blake3Hasher::new()),
        }
    }
    
    fn update(&mut self, data: &[u8]) {
        match self {
            HashState::Md5(h) => h.update(data),
            HashState::Sha1(h) => h.update(data),
            HashState::Sha256(h) => h.update(data),
            HashState::Sha512(h) => h.update(data),
            HashState::Blake3(h) => { h.update(data); },
        }
    }
    
    fn finalize(self) -> String {
        match self {
            HashState::Md5(h) => hex::encode(h.finalize()),
            HashState::Sha1(h) => hex::encode(h.finalize()),
            HashState::Sha256(h) => hex::encode(h.finalize()),
            HashState::Sha512(h) => hex::encode(h.finalize()),
            HashState::Blake3(h) => h.finalize().to_hex().to_string(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up shutdown flag
    let shutdown = Arc::new(AtomicBool::new(false));
    
    let args = Args::parse();

    // Default to blake3 if no algorithm specified
    let algorithm = match args.algorithm {
        Some(alg) => alg,
        None => {
            eprintln!("prhash: using blake3 (no algorithm specified)");
            HashAlgorithm::Blake3
        }
    };

    if args.files.is_empty() {
        anyhow::bail!("No files specified");
    }
    
    // Validate all files exist
    for file in &args.files {
        if !file.exists() {
            anyhow::bail!("File '{}' does not exist", file.display());
        }
        if !file.is_file() {
            anyhow::bail!("'{}' is not a file", file.display());
        }
    }
    
    // Calculate total size
    let mut total_size = 0u64;
    for file in &args.files {
        let metadata = fs::metadata(file)
            .context(format!("Failed to read metadata for '{}'", file.display()))?;
        total_size += metadata.len();
    }
    
    // Set up progress bar
    let pb = ProgressBar::new(total_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta}) {msg}")?
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
    
    // Process each file
    for (idx, file) in args.files.iter().enumerate() {
        pb.set_message(format!("Hashing {} ({}/{})", truncate_path_for_display(file, MAX_FILENAME_DISPLAY_LEN), idx + 1, args.files.len()));
        
        let result = hash_file_with_progress(
            file,
            algorithm,
            &pb,
            paused.clone(),
            shutdown.clone(),
            &mut rx,
        ).await;
        
        match result {
            Ok(hash) => {
                // Print result above progress bar in shasum format
                pb.println(format!("{}  {}", hash, file.display()));
            }
            Err(e) => {
                if e.to_string().contains("cancelled by user") {
                    break;
                }
                pb.println(format!("prhash: {}: {}", file.display(), e));
            }
        }
    }
    
    // Signal shutdown to stop the key listener
    shutdown.store(true, Ordering::SeqCst);
    
    // Disable raw mode
    if raw_mode_enabled {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    
    // Finish progress bar
    pb.finish_and_clear();
    
    // Wait for key task to finish
    let _ = key_task.await;
    
    Ok(())
}

async fn hash_file_with_progress(
    file: &PathBuf,
    algorithm: HashAlgorithm,
    pb: &ProgressBar,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    rx: &mut mpsc::UnboundedReceiver<()>,
) -> Result<String> {
    let mut file_handle = File::open(file)
        .context("Failed to open file")?;
    
    let mut hasher = HashState::new(algorithm);
    let mut buffer = vec![0; BUFFER_SIZE];
    
    loop {
        // Check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            return Err(anyhow::anyhow!("Hash cancelled by user"));
        }
        
        // Check for pause toggle
        if rx.try_recv().is_ok() {
            let was_paused = paused.fetch_xor(true, Ordering::SeqCst);
            if !was_paused {
                pb.set_message(format!("PAUSED - Press space to resume | Hashing {}", truncate_path_for_display(file, MAX_FILENAME_DISPLAY_LEN)));
            } else {
                pb.set_message(format!("Hashing {}", truncate_path_for_display(file, MAX_FILENAME_DISPLAY_LEN)));
            }
        }
        
        // Wait while paused
        while paused.load(Ordering::SeqCst) {
            // Check for shutdown while paused
            if shutdown.load(Ordering::SeqCst) {
                return Err(anyhow::anyhow!("Hash cancelled by user"));
            }
            
            tokio::time::sleep(Duration::from_millis(100)).await;
            
            // Check for unpause
            if rx.try_recv().is_ok() {
                paused.store(false, Ordering::SeqCst);
                pb.set_message(format!("Hashing {}", truncate_path_for_display(file, MAX_FILENAME_DISPLAY_LEN)));
            }
        }
        
        // Read from file
        let bytes_read = match file_handle.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(e) => return Err(e.into()),
        };
        
        // Update hash
        hasher.update(&buffer[..bytes_read]);
        
        pb.inc(bytes_read as u64);
    }
    
    Ok(hasher.finalize())
}

use std::fs;