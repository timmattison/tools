use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::File;
use std::io::{self, BufWriter, Write, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Instant;
use std::thread;
use signal_hook::{consts::SIGINT, iterator::Signals};
use crossterm::{terminal, tty::IsTty};

use crate::{Event, EventType, Recording, get_timestamp};

struct RecordingSession {
    output_path: PathBuf,
    compress: bool,
    recording: Arc<Mutex<Recording>>,
    start_time: Instant,
    should_stop: Arc<AtomicBool>,
}

impl RecordingSession {
    fn new(output_path: PathBuf, compress: bool, width: u16, height: u16, command: String) -> Self {
        let recording = Recording {
            version: 2,
            width,
            height,
            timestamp: get_timestamp(),
            duration: 0.0,
            command,
            title: format!("Terminal recording at {}", chrono::Local::now()),
            env: std::collections::HashMap::new(),
            events: Vec::new(),
        };
        
        Self {
            output_path,
            compress,
            recording: Arc::new(Mutex::new(recording)),
            start_time: Instant::now(),
            should_stop: Arc::new(AtomicBool::new(false)),
        }
    }
    
    fn add_event(&self, event_type: EventType, data: String) {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        let event = Event {
            time: elapsed,
            event_type,
            data,
        };
        
        let mut recording = self.recording.lock().unwrap();
        recording.events.push(event);
        recording.duration = elapsed;
    }
    
    fn save(&self) -> Result<()> {
        let recording = self.recording.lock().unwrap();
        
        if self.compress {
            let file = File::create(&self.output_path)?;
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            serde_json::to_writer(encoder, &*recording)?;
        } else {
            let file = File::create(&self.output_path)?;
            let writer = BufWriter::new(file);
            serde_json::to_writer_pretty(writer, &*recording)?;
        }
        
        Ok(())
    }
    
    fn stop(&self) {
        self.should_stop.store(true, Ordering::Relaxed);
    }
    
    fn should_continue(&self) -> bool {
        !self.should_stop.load(Ordering::Relaxed)
    }
}

pub async fn record(
    output: Option<PathBuf>,
    command: Option<String>,
    append: bool,
    compress: bool,
    _stop_hotkey: String,  // Kept for compatibility but ignored
) -> Result<()> {
    if !io::stdout().is_tty() {
        anyhow::bail!("shellcast record must be run in a terminal");
    }
    
    let output_path = output.unwrap_or_else(|| {
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        PathBuf::from(format!("shellcast_{}.json", timestamp))
    });
    
    if output_path.exists() && !append {
        anyhow::bail!("Output file already exists. Use --append to append to it.");
    }
    
    let (term_width, term_height) = terminal::size()
        .context("Failed to get terminal size")?;
    
    let shell = command.unwrap_or_else(|| {
        std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string())
    });
    
    println!("Recording session to: {}", output_path.display());
    println!("Press Ctrl-C to stop recording, or 'exit' to end the shell session");
    println!();
    
    // Create recording session
    let session = Arc::new(RecordingSession::new(
        output_path.clone(),
        compress,
        term_width,
        term_height,
        shell.clone()
    ));
    
    // Set up PTY
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
    cmd.env("TERM", std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".to_string()));
    
    let mut child = pair.slave.spawn_command(cmd)
        .context("Failed to spawn shell")?;
    
    // Drop the slave to close it
    drop(pair.slave);
    
    // Get readers and writers for the PTY master
    let reader = pair.master.try_clone_reader()
        .context("Failed to clone PTY reader")?;
    let writer = pair.master.take_writer()
        .context("Failed to take PTY writer")?;
    
    // Set up signal handling for graceful shutdown
    let session_for_signal = session.clone();
    let _signal_handle = thread::spawn(move || {
        if let Ok(mut signals) = Signals::new(&[SIGINT]) {
            for sig in signals.forever() {
                if sig == SIGINT {
                    eprintln!("\nReceived interrupt signal, stopping recording...");
                    session_for_signal.stop();
                    break;
                }
            }
        }
    });
    
    // Thread to read from PTY and write to stdout
    let session_reader = session.clone();
    let reader_handle = thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = vec![0; 4096];
        
        while session_reader.should_continue() {
            match reader.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buffer[..n]).to_string();
                    
                    // Write to stdout
                    if let Err(e) = io::stdout().write_all(&buffer[..n]) {
                        eprintln!("Failed to write to stdout: {}", e);
                        break;
                    }
                    if let Err(e) = io::stdout().flush() {
                        eprintln!("Failed to flush stdout: {}", e);
                        break;
                    }
                    
                    // Record the output
                    session_reader.add_event(EventType::Output, data);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(e) => {
                    if !session_reader.should_continue() {
                        break; // Expected when stopping
                    }
                    eprintln!("Error reading from PTY: {}", e);
                    break;
                }
            }
        }
    });
    
    // Thread to read from stdin and write to PTY
    let session_writer = session.clone();
    let writer_handle = thread::spawn(move || {
        let mut writer = writer;  // Make writer mutable in this scope
        let mut buffer = vec![0; 4096];
        
        while session_writer.should_continue() {
            match io::stdin().read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = &buffer[..n];
                    
                    // Record the input
                    let input_str = String::from_utf8_lossy(data).to_string();
                    session_writer.add_event(EventType::Input, input_str);
                    
                    // Forward to PTY
                    if let Err(e) = writer.write_all(data) {
                        eprintln!("Failed to write to PTY: {}", e);
                        break;
                    }
                    if let Err(e) = writer.flush() {
                        eprintln!("Failed to flush PTY: {}", e);
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                Err(e) => {
                    eprintln!("Error reading from stdin: {}", e);
                    break;
                }
            }
        }
        
        // Signal that input has ended
        session_writer.stop();
    });
    
    // Wait for child process to exit
    let exit_status = child.wait()
        .context("Failed to wait for child process")?;
    
    // Stop recording
    session.stop();
    
    // Wait for threads to finish
    let _ = reader_handle.join();
    let _ = writer_handle.join();
    
    // Save the recording
    session.save()
        .context("Failed to save recording")?;
    
    println!("\nRecording saved to: {}", output_path.display());
    println!("Exit status: {}", exit_status);
    
    Ok(())
}