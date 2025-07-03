use anyhow::{Context, Result};
use crossterm::{terminal, tty::IsTty};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::File;
use std::io::{BufWriter, Write, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::thread;

use crate::{Event, EventType, Recording, get_timestamp};

pub async fn record(
    output: Option<PathBuf>,
    command: Option<String>,
    append: bool,
    compress: bool,
) -> Result<()> {
    if !std::io::stdout().is_tty() {
        anyhow::bail!("beta record must be run in a terminal");
    }
    
    let output_path = output.unwrap_or_else(|| {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("beta_{}.json", timestamp))
    });
    
    if output_path.exists() && !append {
        anyhow::bail!("Output file already exists. Use --append to append to it.");
    }
    
    let (term_width, term_height) = terminal::size()
        .context("Failed to get terminal size")?;
    
    let shell = command.unwrap_or_else(|| {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string())
    });
    
    let events = Arc::new(Mutex::new(Vec::new()));
    let start_time = Instant::now();
    let start_timestamp = get_timestamp();
    
    println!("Recording session to: {}", output_path.display());
    println!("Press Ctrl-D or type 'exit' to finish recording");
    println!();
    
    let pty_system = native_pty_system();
    
    let pty_size = PtySize {
        rows: term_height,
        cols: term_width,
        pixel_width: 0,
        pixel_height: 0,
    };
    
    let pair = pty_system
        .openpty(pty_size)
        .context("Failed to open PTY")?;
    
    let mut cmd = CommandBuilder::new(&shell);
    cmd.cwd(std::env::current_dir()?);
    
    let mut child = pair.slave.spawn_command(cmd)
        .context("Failed to spawn shell")?;
    
    // Drop the slave to close it
    drop(pair.slave);
    
    let reader = pair.master.try_clone_reader()
        .context("Failed to clone PTY reader")?;
    let mut writer = pair.master.take_writer()
        .context("Failed to take PTY writer")?;
    
    // Thread to read from PTY and write to stdout
    let events_clone = events.clone();
    let reader_thread = thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = vec![0; 4096];
        
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let elapsed = start_time.elapsed().as_secs_f64();
                    
                    std::io::stdout().write_all(&buffer[..n]).unwrap();
                    std::io::stdout().flush().unwrap();
                    
                    events_clone.lock().unwrap().push(Event {
                        time: elapsed,
                        event_type: EventType::Output,
                        data,
                    });
                }
                Err(_) => break,
            }
        }
    });
    
    // Thread to read from stdin and write to PTY
    let events_clone = events.clone();
    let writer_thread = thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = vec![0; 1024];
        
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let data = buffer[..n].to_vec();
                    let input_str = String::from_utf8_lossy(&data).to_string();
                    let elapsed = start_time.elapsed().as_secs_f64();
                    
                    events_clone.lock().unwrap().push(Event {
                        time: elapsed,
                        event_type: EventType::Input,
                        data: input_str,
                    });
                    
                    if writer.write_all(&data).is_err() {
                        break;
                    }
                    writer.flush().ok();
                }
                Err(_) => break,
            }
        }
    });
    
    // Wait for threads to complete
    let _ = reader_thread.join();
    let _ = writer_thread.join();
    let _ = child.wait();
    
    let duration = start_time.elapsed().as_secs_f64();
    let events = Arc::try_unwrap(events)
        .map(|mutex| mutex.into_inner().unwrap())
        .unwrap_or_else(|arc| arc.lock().unwrap().clone());
    
    let recording = Recording {
        version: 2,
        width: term_width,
        height: term_height,
        timestamp: start_timestamp,
        duration,
        command: shell,
        title: format!("Terminal recording at {}", chrono::Local::now()),
        env: std::collections::HashMap::new(),
        events,
    };
    
    let file = File::create(&output_path)
        .context("Failed to create output file")?;
    
    if compress {
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let writer = BufWriter::new(encoder);
        serde_json::to_writer_pretty(writer, &recording)
            .context("Failed to write compressed recording")?;
    } else {
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &recording)
            .context("Failed to write recording")?;
    }
    
    println!("\nRecording saved to: {}", output_path.display());
    println!("Duration: {:.1}s", duration);
    println!("Events: {}", recording.events.len());
    
    Ok(())
}