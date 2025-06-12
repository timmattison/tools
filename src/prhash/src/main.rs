use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode},
};
use dialoguer::console::Term;
use indicatif::{ProgressBar, ProgressStyle};
use md5::{Digest, Md5};
use num_format::{Locale, ToFormattedString};
use sha1::Sha1;
use sha2::{Sha256, Sha512};
use std::{
    fs::File,
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use thiserror::Error;

/// A tool to hash files with progress display
#[derive(Parser)]
#[clap(name = "prhash", about = "Hash files with progress display")]
struct Args {
    /// Hash algorithm to use (md5, sha1, sha256, sha512, blake3)
    hash_type: String,

    /// Input file(s) to hash
    input_files: Vec<String>,
}

#[derive(Error, Debug)]
enum HashError {
    #[error("Invalid hash type")]
    InvalidHashType,

    #[error("Error reading file")]
    FileReadError(#[from] io::Error),
}

// Define a trait for hashing
trait Hasher {
    fn update(&mut self, data: &[u8]);
    fn finalize(&self) -> String;
    fn reset(&mut self);
}

// Implement the Hasher trait for each hash algorithm
struct Md5Hasher(Md5);

impl Hasher for Md5Hasher {
    fn update(&mut self, data: &[u8]) {
        use md5::Digest;
        self.0.update(data);
    }

    fn finalize(&self) -> String {
        use md5::Digest;
        let result = self.0.clone().finalize();
        format!("{:x}", result)
    }

    fn reset(&mut self) {
        self.0 = Md5::new();
    }
}

struct Sha1Hasher(Sha1);

impl Hasher for Sha1Hasher {
    fn update(&mut self, data: &[u8]) {
        use sha1::Digest;
        self.0.update(data);
    }

    fn finalize(&self) -> String {
        use sha1::Digest;
        let result = self.0.clone().finalize();
        format!("{:x}", result)
    }

    fn reset(&mut self) {
        self.0 = Sha1::new();
    }
}

struct Sha256Hasher(Sha256);

impl Hasher for Sha256Hasher {
    fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        self.0.update(data);
    }

    fn finalize(&self) -> String {
        use sha2::Digest;
        let result = self.0.clone().finalize();
        format!("{:x}", result)
    }

    fn reset(&mut self) {
        self.0 = Sha256::new();
    }
}

struct Sha512Hasher(Sha512);

impl Hasher for Sha512Hasher {
    fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        self.0.update(data);
    }

    fn finalize(&self) -> String {
        use sha2::Digest;
        let result = self.0.clone().finalize();
        format!("{:x}", result)
    }

    fn reset(&mut self) {
        self.0 = Sha512::new();
    }
}

struct Blake3Hasher(blake3::Hasher);

impl Hasher for Blake3Hasher {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    fn finalize(&self) -> String {
        self.0.clone().finalize().to_hex().to_string()
    }

    fn reset(&mut self) {
        self.0 = blake3::Hasher::new();
    }
}

fn create_hasher(hash_type: &str) -> Result<Box<dyn Hasher>> {
    match hash_type {
        "md5" => Ok(Box::new(Md5Hasher(Md5::new()))),
        "sha1" => Ok(Box::new(Sha1Hasher(Sha1::new()))),
        "sha256" => Ok(Box::new(Sha256Hasher(Sha256::new()))),
        "sha512" => Ok(Box::new(Sha512Hasher(Sha512::new()))),
        "blake3" => Ok(Box::new(Blake3Hasher(blake3::Hasher::new()))),
        _ => Err(HashError::InvalidHashType.into()),
    }
}

fn valid_hash_types() -> Vec<String> {
    vec![
        "md5".to_string(),
        "sha1".to_string(),
        "sha256".to_string(),
        "sha512".to_string(),
        "blake3".to_string(),
    ]
}

fn print_valid_hash_types() {
    println!("Valid hash types are:");
    for hash_type in valid_hash_types() {
        println!("  {}", hash_type);
    }
}

fn calculate_throughput(start_time: Instant, end_time: Instant, processed_bytes: u64) -> u64 {
    let duration_ms = end_time.duration_since(start_time).as_millis();
    if duration_ms == 0 {
        0
    } else {
        (processed_bytes as f64 / duration_ms as f64 * 1000.0) as u64
    }
}

fn format_throughput(throughput: u64) -> String {
    const MB: u64 = 1_000_000;
    const KB: u64 = 1_000;

    if throughput > MB {
        format!("{} MB/s", throughput / MB)
    } else if throughput > KB {
        format!("{} KB/s", throughput / KB)
    } else {
        format!("{} B/s", throughput)
    }
}

fn hash_file(path: &Path, hash_type: &str) -> Result<()> {
    Term::stdout();
    let mut hasher = create_hasher(hash_type)?;

    // Open the file
    let file =
        File::open(path).with_context(|| format!("Failed to open file {}", path.display()))?;
    let file_size = file.metadata()?.len();
    let file_name = path.display();

    // Create a buffered reader
    let mut reader = BufReader::new(file);

    // Create a progress bar
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut buffer = [0; 16 * 1024 * 1024]; // 16MB buffer
    let mut total_read = 0;
    let start_time = Instant::now();
    let mut paused = false;

    // Enable raw mode for key detection
    enable_raw_mode()?;

    // Clear screen and show initial message
    println!("Hashing {} with {}", file_name, hash_type);
    println!("Press SPACE to pause/resume, CTRL-C to abort");

    loop {
        // Check for key presses
        while event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c')
                            if key.modifiers.contains(event::KeyModifiers::CONTROL) =>
                        {
                            pb.finish_and_clear();
                            disable_raw_mode()?;
                            println!("Hashing aborted");
                            return Ok(());
                        }
                        KeyCode::Char(' ') => {
                            paused = !paused;
                            if paused {
                                pb.suspend(|| {
                                    println!("\nPaused - press SPACE to continue");
                                });
                            } else {
                                println!("Resuming hash calculation...");
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        if paused {
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        match reader.read(&mut buffer) {
            Ok(0) => break, // End of file
            Ok(bytes_read) => {
                hasher.update(&buffer[0..bytes_read]);
                total_read += bytes_read as u64;
                pb.set_position(total_read);

                // Show throughput every 500ms
                if total_read % (32 * 1024 * 1024) == 0 {
                    let throughput = calculate_throughput(start_time, Instant::now(), total_read);
                    pb.set_message(format!(
                        "{} / {} - {}",
                        total_read.to_formatted_string(&Locale::en),
                        file_size.to_formatted_string(&Locale::en),
                        format_throughput(throughput)
                    ));
                }
            }
            Err(e) => {
                pb.finish_and_clear();
                disable_raw_mode()?;
                return Err(e.into());
            }
        }
    }

    // Finalize and print the hash
    let hash = hasher.finalize();

    pb.finish_and_clear();
    disable_raw_mode()?;

    // Print the final result
    println!("{}  {}", hash, file_name);

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.input_files.is_empty() {
        println!("Missing required arguments.");
        println!("Usage:");
        println!("  prhash <hash type> <input file(s)> ...");
        println!();
        print_valid_hash_types();
        std::process::exit(1);
    }

    let hash_type = &args.hash_type;

    if !valid_hash_types().contains(&hash_type.to_string()) {
        println!("Invalid hash type.");
        print_valid_hash_types();
        std::process::exit(1);
    }

    for input_file in &args.input_files {
        let path = PathBuf::from(input_file);

        if let Err(e) = hash_file(&path, hash_type) {
            eprintln!("Error hashing file {}: {}", path.display(), e);
            std::process::exit(1);
        }
    }

    Ok(())
}
