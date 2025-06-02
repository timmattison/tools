use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
};
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod hash;
mod ui;
mod app;

use app::{App, AppState};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Hash type to use
    #[arg(value_name = "HASH_TYPE")]
    hash_type: String,
    
    /// Input file(s) to hash
    #[arg(value_name = "INPUT_FILES")]
    input_files: Vec<PathBuf>,
}

fn print_valid_hash_types() {
    println!("Valid hash types are:");
    for hash_type in &["md5", "sha1", "sha256", "sha512", "blake3"] {
        println!("  {}", hash_type);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    
    if args.input_files.is_empty() {
        println!("Missing required arguments.");
        println!("Usage:");
        println!("  prhash <hash type> <input file(s)> ...");
        println!();
        print_valid_hash_types();
        std::process::exit(1);
    }
    
    if !hash::is_valid_hash_type(&args.hash_type) {
        println!("Invalid hash type.");
        print_valid_hash_types();
        std::process::exit(1);
    }
    
    for input_file in args.input_files {
        if !input_file.exists() {
            eprintln!("Error: File not found: {}", input_file.display());
            std::process::exit(1);
        }
        
        if let Err(e) = process_file(&args.hash_type, &input_file).await {
            eprintln!("Error processing {}: {}", input_file.display(), e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}

async fn process_file(hash_type: &str, input_file: &PathBuf) -> Result<()> {
    // Try TUI mode first, fallback to simple mode if no TTY
    if std::io::stderr().is_terminal() {
        process_file_tui(hash_type, input_file).await
    } else {
        process_file_simple(hash_type, input_file).await
    }
}

async fn process_file_simple(hash_type: &str, input_file: &PathBuf) -> Result<()> {
    use crate::hash::{hash_file, HashMessage};
    use tokio::sync::mpsc;
    
    let file_metadata = std::fs::metadata(input_file)?;
    let file_size = file_metadata.len();
    
    let (progress_sender, mut progress_receiver) = mpsc::unbounded_channel();
    let (_pause_sender, pause_receiver) = mpsc::unbounded_channel();
    
    // Start the hash calculation task
    let file_path = input_file.clone();
    let hash_type_owned = hash_type.to_string();
    let hash_task = tokio::spawn(async move {
        hash_file(&file_path, &hash_type_owned, progress_sender, pause_receiver).await
    });
    
    // Show progress for large files
    let show_progress = file_size > 10 * 1024 * 1024; // Show progress for files > 10MB
    if show_progress {
        eprintln!("Hashing {} ({} bytes) with {}...", 
                 input_file.display(), 
                 file_size, 
                 hash_type.to_uppercase());
    }
    
    // Wait for completion
    let mut hash_result = None;
    let mut last_progress = 0u64;
    
    while let Some(message) = progress_receiver.recv().await {
        match message {
            HashMessage::Progress(progress) => {
                if show_progress && progress.bytes_processed >= last_progress + (10 * 1024 * 1024) {
                    let percent = (progress.bytes_processed as f64 / file_size as f64) * 100.0;
                    eprintln!("Progress: {:.1}% ({} / {} bytes)", 
                             percent, 
                             progress.bytes_processed,
                             file_size);
                    last_progress = progress.bytes_processed;
                }
            }
            HashMessage::Finished(hash_value) => {
                hash_result = Some(hash_value);
                break;
            }
            HashMessage::Error(error_msg) => {
                anyhow::bail!("Hash calculation failed: {}", error_msg);
            }
        }
    }
    
    // Wait for task completion
    hash_task.await??;
    
    if let Some(hash_value) = hash_result {
        println!("{}  {}", hash_value, input_file.display());
    }
    
    Ok(())
}

async fn process_file_tui(hash_type: &str, input_file: &PathBuf) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(hash_type, input_file).await?;
    let _last_tick = Instant::now();
    let tick_rate = Duration::from_millis(250);
    
    let result = run_app(&mut terminal, &mut app, tick_rate).await;
    
    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    
    match result {
        Ok(should_exit_abnormally) => {
            if should_exit_abnormally {
                std::process::exit(1);
            }
            
            if let Some(hash_value) = app.get_hash_result() {
                println!("{}  {}", hash_value, input_file.display());
            }
        }
        Err(e) => {
            eprintln!("Application error: {}", e);
            std::process::exit(1);
        }
    }
    
    Ok(())
}

async fn run_app<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    tick_rate: Duration,
) -> Result<bool> {
    let mut last_tick = Instant::now();
    
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
            
        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                            return Ok(true); // Abnormal exit
                        }
                        KeyCode::Char(' ') => {
                            app.toggle_pause();
                        }
                        _ => {}
                    }
                }
            }
        }
        
        if last_tick.elapsed() >= tick_rate {
            app.tick().await;
            last_tick = Instant::now();
        }
        
        match app.state() {
            AppState::Error(_) => return Ok(true), // Abnormal exit
            AppState::Finished => return Ok(false), // Normal exit
            _ => {}
        }
    }
}