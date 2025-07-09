use anyhow::{Context, Result};
use crossterm::{terminal, tty::IsTty, event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers}};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::fs::File;
use std::io::{BufWriter, Write, Read};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Instant, Duration};
use std::thread;
use tokio_stream::StreamExt;

use crate::{Event, EventType, Recording, get_timestamp};

fn parse_hotkey(hotkey_str: &str) -> (KeyCode, KeyModifiers) {
    match hotkey_str.to_lowercase().as_str() {
        "ctrl-end" => (KeyCode::End, KeyModifiers::CONTROL),
        "ctrl-]" => (KeyCode::Char(']'), KeyModifiers::CONTROL),
        "f12" => (KeyCode::F(12), KeyModifiers::NONE),
        "ctrl-\\" | "ctrl-\\\\" => (KeyCode::Char('\\'), KeyModifiers::CONTROL),
        "ctrl-c" => (KeyCode::Char('c'), KeyModifiers::CONTROL),
        _ => {
            eprintln!("Warning: Unknown hotkey '{}', defaulting to Ctrl-End", hotkey_str);
            (KeyCode::End, KeyModifiers::CONTROL)
        }
    }
}

fn is_stop_hotkey(key_event: &KeyEvent, stop_key: &(KeyCode, KeyModifiers)) -> bool {
    // Check the configured hotkey with modifiers
    if key_event.code == stop_key.0 && key_event.modifiers.contains(stop_key.1) {
        return true;
    }
    
    // Handle raw control characters that terminals might send
    // Check if we're looking for CTRL-] and got ASCII 29
    if matches!(stop_key, (KeyCode::Char(']'), mods) if mods.contains(KeyModifiers::CONTROL)) {
        if let KeyCode::Char(c) = key_event.code {
            // Check for ASCII 29 (Group Separator) which is CTRL-]
            if c == '\x1d' || c as u8 == 29 {
                return true;
            }
        }
    }
    
    // Check if we're looking for CTRL-\ and got ASCII 28
    if matches!(stop_key, (KeyCode::Char('\\'), mods) if mods.contains(KeyModifiers::CONTROL)) {
        if let KeyCode::Char(c) = key_event.code {
            // Check for ASCII 28 (File Separator) which is CTRL-\
            if c == '\x1c' || c as u8 == 28 {
                return true;
            }
        }
    }
    
    // Check if we're looking for CTRL-C and got ASCII 3
    if matches!(stop_key, (KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL)) {
        if let KeyCode::Char(c) = key_event.code {
            // Check for ASCII 3 (End of Text) which is CTRL-C
            if c == '\x03' || c as u8 == 3 {
                return true;
            }
        }
    }
    
    false
}

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
    stop_hotkey: String,
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
    
    let stop_key = parse_hotkey(&stop_hotkey);
    let hotkey_display = match stop_hotkey.as_str() {
        "ctrl-end" => "Ctrl-End",
        "ctrl-]" => "Ctrl-]",
        "f12" => "F12",
        "ctrl-\\" | "ctrl-\\\\" => "Ctrl-\\",
        "ctrl-c" => "Ctrl-C",
        _ => "Ctrl-End",
    };
    
    println!("Recording session to: {}", output_path.display());
    println!("Press {} to stop recording gracefully, or 'exit' to end the shell session", hotkey_display);
    println!();
    
    // Create recording session
    let session = Arc::new(RecordingSession::new(
        output_path.clone(),
        compress,
        term_width,
        term_height,
        shell.clone()
    ));
    
    // Enable crossterm events before entering raw mode
    // This ensures we can capture keyboard events properly
    
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
    
    let child = pair.slave.spawn_command(cmd)
        .context("Failed to spawn shell")?;
    
    // Drop the slave to close it
    drop(pair.slave);
    
    // Store master in Arc<Mutex> so we can close it later
    let master = Arc::new(Mutex::new(Some(pair.master)));
    
    let reader = {
        let master_lock = master.lock().unwrap();
        master_lock.as_ref().unwrap().try_clone_reader()
            .context("Failed to clone PTY reader")?
    };
    
    let mut writer = {
        let mut master_lock = master.lock().unwrap();
        master_lock.as_mut().unwrap().take_writer()
            .context("Failed to take PTY writer")?
    };
    
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
    
    // Thread to handle keyboard events with hotkey detection
    let session_writer = session.clone();
    let stop_key_clone = stop_key.clone();
    let hotkey_display_clone = hotkey_display.to_string();
    let master_clone = master.clone();
    let writer_handle = tokio::spawn(async move {
        let mut event_stream = event::EventStream::new();
        
        while session_writer.should_continue() {
            // Use a timeout to check should_continue periodically
            let timeout_duration = Duration::from_millis(100);
            
            match tokio::time::timeout(timeout_duration, event_stream.next()).await {
                Ok(Some(Ok(CrosstermEvent::Key(key_event)))) => {
                    // Debug logging if BETA_DEBUG env var is set
                    if std::env::var("BETA_DEBUG").is_ok() {
                        eprintln!("DEBUG: Key event - code: {:?}, modifiers: {:?}, char value: {}", 
                            key_event.code, 
                            key_event.modifiers,
                            if let KeyCode::Char(c) = key_event.code { c as u8 } else { 0 }
                        );
                    }
                    
                    // Check for stop hotkey
                    if is_stop_hotkey(&key_event, &stop_key_clone) {
                        // Disable raw mode before printing to fix terminal output
                        let _ = terminal::disable_raw_mode();
                        
                        eprintln!("\nStop hotkey detected ({}), stopping recording...", hotkey_display_clone);
                        session_writer.stop();
                        
                        // Close the PTY master to signal EOF to reader thread
                        if let Ok(mut master_lock) = master_clone.lock() {
                            *master_lock = None; // Drop the master
                        }
                        
                        // Flush output to ensure messages are displayed
                        let _ = std::io::stderr().flush();
                        break;
                    }
                    
                    // Convert key event to bytes for PTY forwarding
                    let data = match key_event.code {
                        KeyCode::Char(c) => {
                            if key_event.modifiers.contains(KeyModifiers::CONTROL) {
                                // Handle control characters
                                let ctrl_char = if c.is_ascii_alphabetic() {
                                    (c.to_ascii_uppercase() as u8 - b'A' + 1) as char
                                } else {
                                    // Handle special control characters
                                    match c {
                                        ']' => '\x1d' as char, // ASCII 29 (Group Separator)
                                        '\\' => '\x1c' as char, // ASCII 28 (File Separator)
                                        _ => c,
                                    }
                                };
                                vec![ctrl_char as u8]
                            } else {
                                c.to_string().into_bytes()
                            }
                        }
                        KeyCode::Enter => vec![b'\r'],
                        KeyCode::Tab => vec![b'\t'],
                        KeyCode::Backspace => vec![b'\x7f'],
                        KeyCode::Esc => vec![b'\x1b'],
                        KeyCode::Up => b"\x1b[A".to_vec(),
                        KeyCode::Down => b"\x1b[B".to_vec(),
                        KeyCode::Right => b"\x1b[C".to_vec(),
                        KeyCode::Left => b"\x1b[D".to_vec(),
                        KeyCode::Home => b"\x1b[H".to_vec(),
                        KeyCode::End => b"\x1b[F".to_vec(),
                        KeyCode::PageUp => b"\x1b[5~".to_vec(),
                        KeyCode::PageDown => b"\x1b[6~".to_vec(),
                        KeyCode::Delete => b"\x1b[3~".to_vec(),
                        KeyCode::Insert => b"\x1b[2~".to_vec(),
                        KeyCode::F(n) => {
                            match n {
                                1..=4 => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
                                5..=12 => format!("\x1b[{}{}", n + 10, if n <= 5 { "~" } else { "~" }).into_bytes(),
                                _ => continue,
                            }
                        }
                        _ => continue, // Skip other key types
                    };
                    
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
                Ok(Some(Ok(_))) => {
                    // Ignore other events (mouse, resize, etc.)
                }
                Ok(Some(Err(e))) => {
                    eprintln!("Error reading events: {}", e);
                    break;
                }
                Ok(None) => {
                    // Event stream ended
                    break;
                }
                Err(_) => {
                    // Timeout - continue loop to check should_continue
                }
            }
        }
    });
    
    // Wrap child in Arc<Mutex> so we can kill it when needed
    let child = Arc::new(Mutex::new(child));
    
    // Wait for child process or stop signal
    let session_monitor = session.clone();
    let child_clone = child.clone();
    let child_handle = thread::spawn(move || {
        if let Ok(mut child) = child_clone.lock() {
            let _ = child.wait();
        }
        session_monitor.stop();
    });
    
    // Wait for keyboard handler to complete
    let _ = writer_handle.await;
    
    // Close PTY master to ensure reader thread exits
    if let Ok(mut master_lock) = master.lock() {
        *master_lock = None; // Drop the master
    }
    
    // Kill the child process if it's still running
    if let Ok(mut child) = child.lock() {
        let _ = child.kill();
    }
    
    // Wait for threads to complete
    let _ = reader_handle.join();
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
    
    // Ensure terminal is restored (RawModeGuard will also do this on drop)
    let _ = terminal::disable_raw_mode();
    
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
        // Flush output to ensure terminal is properly restored
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
    }
}