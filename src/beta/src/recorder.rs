use anyhow::{Context, Result};
use crossterm::{terminal, tty::IsTty};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::File;
use std::io::{BufWriter, Write, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Instant;
use std::thread;
use signal_hook::{consts::SIGINT, iterator::Signals};

use crate::{Event, EventType, Recording, get_timestamp};

struct RecordingSession {
    events: Arc<Mutex<Vec<Event>>>,
    start_time: Instant,
    start_timestamp: f64,
    output_path: PathBuf,
    compress: bool,
    recording: Recording,
    should_stop: Arc<AtomicBool>,
}

impl RecordingSession {
    fn new(output_path: PathBuf, compress: bool, width: u16, height: u16, shell: String) -> Self {
        let start_time = Instant::now();
        let start_timestamp = get_timestamp();
        
        let recording = Recording {
            version: 2,
            width,
            height,
            timestamp: start_timestamp,
            duration: 0.0,
            command: shell,
            title: format!("Terminal recording at {}", chrono::Local::now()),
            env: std::collections::HashMap::new(),
            events: Vec::new(),
        };

        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            start_time,
            start_timestamp,
            output_path,
            compress,
            recording,
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
        
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    fn save_recording(&self) -> Result<()> {
        let duration = self.start_time.elapsed().as_secs_f64();
        let events = self.events.lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock events"))?
            .clone();

        let mut recording = self.recording.clone();
        recording.duration = duration;
        recording.events = events;

        let file = File::create(&self.output_path)
            .context("Failed to create output file")?;

        if self.compress {
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let writer = BufWriter::new(encoder);
            serde_json::to_writer_pretty(writer, &recording)
                .context("Failed to write compressed recording")?;
        } else {
            let writer = BufWriter::new(file);
            serde_json::to_writer_pretty(writer, &recording)
                .context("Failed to write recording")?;
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
    
    println!("Recording session to: {}", output_path.display());
    println!("Press Ctrl-C to stop recording gracefully, or 'exit' to end the shell session");
    println!();
    
    // Create recording session
    let session = Arc::new(RecordingSession::new(
        output_path.clone(),
        compress,
        term_width,
        term_height,
        shell.clone()
    ));
    
    // Set up signal handling
    let session_clone = session.clone();
    let mut signals = Signals::new(&[SIGINT])
        .context("Failed to register signal handler")?;
    
    thread::spawn(move || {
        for sig in signals.forever() {
            match sig {
                SIGINT => {
                    eprintln!("\nReceived interrupt signal, saving recording...");
                    session_clone.stop();
                    if let Err(e) = session_clone.save_recording() {
                        eprintln!("Failed to save recording: {}", e);
                    } else {
                        eprintln!("Recording saved to: {}", session_clone.output_path.display());
                    }
                    std::process::exit(0);
                }
                _ => {}
            }
        }
    });
    
    // Enable raw mode for proper keyboard capture
    terminal::enable_raw_mode()
        .context("Failed to enable raw mode")?;
    
    // Ensure raw mode is disabled on exit
    let _raw_mode_guard = RawModeGuard;
    
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
    
    let mut child = pair.slave.spawn_command(cmd)
        .context("Failed to spawn shell")?;
    
    // Drop the slave to close it
    drop(pair.slave);
    
    let reader = pair.master.try_clone_reader()
        .context("Failed to clone PTY reader")?;
    let mut writer = pair.master.take_writer()
        .context("Failed to take PTY writer")?;
    
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
                    
                    // Write to stdout (in raw mode, we need to handle this carefully)
                    if let Err(e) = std::io::stdout().write_all(&buffer[..n]) {
                        eprintln!("Failed to write to stdout: {}", e);
                        break;
                    }
                    if let Err(e) = std::io::stdout().flush() {
                        eprintln!("Failed to flush stdout: {}", e);
                        break;
                    }
                    
                    // Record the output
                    session_reader.add_event(EventType::Output, data);
                }
                Err(e) => {
                    eprintln!("Error reading from PTY: {}", e);
                    break;
                }
            }
        }
    });
    
    // Thread to read from stdin and write to PTY (with raw mode support)
    let session_writer = session.clone();
    let writer_handle = thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = vec![0; 1024];
        
        while session_writer.should_continue() {
            match stdin.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = buffer[..n].to_vec();
                    let input_str = String::from_utf8_lossy(&data).to_string();
                    
                    // Record the input
                    session_writer.add_event(EventType::Input, input_str);
                    
                    // Forward to PTY
                    if let Err(e) = writer.write_all(&data) {
                        eprintln!("Failed to write to PTY: {}", e);
                        break;
                    }
                    if let Err(e) = writer.flush() {
                        eprintln!("Failed to flush PTY: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Error reading from stdin: {}", e);
                    break;
                }
            }
        }
    });
    
    // Wait for child process or stop signal
    let session_monitor = session.clone();
    let child_handle = thread::spawn(move || {
        let _ = child.wait();
        session_monitor.stop();
    });
    
    // Wait for all threads to complete
    let _ = reader_handle.join();
    let _ = writer_handle.join();
    let _ = child_handle.join();
    
    // Save the recording
    if let Err(e) = session.save_recording() {
        eprintln!("Failed to save recording: {}", e);
        return Err(e);
    }
    
    let duration = session.start_time.elapsed().as_secs_f64();
    let events_count = session.events.lock()
        .map(|events| events.len())
        .unwrap_or(0);
    
    println!("\nRecording saved to: {}", output_path.display());
    println!("Duration: {:.1}s", duration);
    println!("Events: {}", events_count);
    
    Ok(())
}

// RAII guard to ensure raw mode is disabled
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if let Err(e) = terminal::disable_raw_mode() {
            eprintln!("Failed to disable raw mode: {}", e);
        }
    }
}