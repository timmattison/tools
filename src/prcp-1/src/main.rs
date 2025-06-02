use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame, Terminal,
};
use std::{
    env,
    fs::File,
    io::{self, BufReader, BufWriter, Read, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::mpsc,
    time::sleep,
};

const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer like the Go version

#[derive(Debug)]
struct CopyProgress {
    bytes_copied: u64,
    total_bytes: u64,
    throughput: u64, // bytes per second
    is_finished: bool,
    error: Option<String>,
}

struct App {
    source_path: String,
    destination_path: String,
    progress: CopyProgress,
    is_paused: Arc<AtomicBool>,
    should_quit: bool,
}

impl App {
    fn new(source_path: String, destination_path: String, total_bytes: u64) -> Self {
        Self {
            source_path,
            destination_path,
            progress: CopyProgress {
                bytes_copied: 0,
                total_bytes,
                throughput: 0,
                is_finished: false,
                error: None,
            },
            is_paused: Arc::new(AtomicBool::new(false)),
            should_quit: false,
        }
    }

    fn update_progress(&mut self, progress: CopyProgress) {
        self.progress = progress;
        if self.progress.is_finished || self.progress.error.is_some() {
            self.should_quit = true;
        }
    }

    fn toggle_pause(&self) {
        let current = self.is_paused.load(Ordering::Relaxed);
        self.is_paused.store(!current, Ordering::Relaxed);
    }

    fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::Relaxed)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Missing required arguments.");
        eprintln!("Usage:");
        eprintln!("  prcp <source file> <destination file>");
        std::process::exit(1);
    }

    let source_path = &args[1];
    let destination_path = &args[2];

    // Validate source file exists and get size
    let source_metadata = std::fs::metadata(source_path)
        .with_context(|| format!("Failed to read source file metadata: {}", source_path))?;
    
    if !source_metadata.is_file() {
        anyhow::bail!("Source path is not a file: {}", source_path);
    }

    let total_bytes = source_metadata.len();

    // Validate destination path
    if let Some(parent) = Path::new(destination_path).parent() {
        if !parent.exists() {
            anyhow::bail!("Destination directory does not exist: {}", parent.display());
        }
    }

    let app = Arc::new(tokio::sync::Mutex::new(App::new(
        source_path.clone(),
        destination_path.clone(),
        total_bytes,
    )));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<CopyProgress>();

    // Start copy task
    let copy_app = app.clone();
    let copy_source = source_path.clone();
    let copy_destination = destination_path.clone();
    
    tokio::spawn(async move {
        if let Err(e) = copy_file(&copy_source, &copy_destination, total_bytes, progress_tx, copy_app).await {
            eprintln!("Copy failed: {}", e);
        }
    });

    // Main UI loop
    let result = run_ui(&mut terminal, app, &mut progress_rx).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn copy_file(
    source_path: &str,
    destination_path: &str,
    total_bytes: u64,
    progress_tx: mpsc::UnboundedSender<CopyProgress>,
    app: Arc<tokio::sync::Mutex<App>>,
) -> Result<()> {
    let source_file = File::open(source_path)
        .with_context(|| format!("Failed to open source file: {}", source_path))?;
    let destination_file = File::create(destination_path)
        .with_context(|| format!("Failed to create destination file: {}", destination_path))?;

    let mut reader = BufReader::with_capacity(BUFFER_SIZE, source_file);
    let mut writer = BufWriter::with_capacity(BUFFER_SIZE, destination_file);

    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut bytes_copied = 0u64;
    let start_time = Instant::now();

    loop {
        // Check if we should pause
        loop {
            let app_guard = app.lock().await;
            if !app_guard.is_paused() || app_guard.should_quit {
                let should_quit = app_guard.should_quit;
                drop(app_guard);
                if should_quit {
                    return Ok(());
                }
                break;
            }
            drop(app_guard);
            sleep(Duration::from_millis(100)).await;
        }

        match reader.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(bytes_read) => {
                writer.write_all(&buffer[..bytes_read])
                    .with_context(|| "Failed to write to destination file")?;
                
                bytes_copied += bytes_read as u64;
                let elapsed = start_time.elapsed();
                let throughput = if elapsed.as_secs() > 0 {
                    bytes_copied / elapsed.as_secs()
                } else {
                    0
                };

                let progress = CopyProgress {
                    bytes_copied,
                    total_bytes,
                    throughput,
                    is_finished: false,
                    error: None,
                };

                if progress_tx.send(progress).is_err() {
                    break; // UI has been closed
                }
            }
            Err(e) => {
                let progress = CopyProgress {
                    bytes_copied,
                    total_bytes,
                    throughput: 0,
                    is_finished: false,
                    error: Some(format!("Read error: {}", e)),
                };
                let _ = progress_tx.send(progress);
                return Err(e.into());
            }
        }
    }

    writer.flush().with_context(|| "Failed to flush destination file")?;

    // Send final progress
    let final_progress = CopyProgress {
        bytes_copied,
        total_bytes,
        throughput: if start_time.elapsed().as_secs() > 0 {
            bytes_copied / start_time.elapsed().as_secs()
        } else {
            0
        },
        is_finished: true,
        error: None,
    };

    let _ = progress_tx.send(final_progress);
    Ok(())
}

async fn run_ui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: Arc<tokio::sync::Mutex<App>>,
    progress_rx: &mut mpsc::UnboundedReceiver<CopyProgress>,
) -> Result<()> {
    loop {
        // Handle events and progress updates
        tokio::select! {
            // Handle terminal events
            _ = handle_events(app.clone()) => {
                let app_guard = app.lock().await;
                if app_guard.should_quit {
                    break;
                }
            }
            
            // Handle progress updates
            progress = progress_rx.recv() => {
                if let Some(progress) = progress {
                    let mut app_guard = app.lock().await;
                    app_guard.update_progress(progress);
                    let should_quit = app_guard.should_quit;
                    drop(app_guard);
                    
                    if should_quit {
                        break;
                    }
                }
            }
        }

        // Draw UI
        {
            let app_guard = app.lock().await;
            terminal.draw(|f| ui(f, &*app_guard))?;
        }
    }

    Ok(())
}

async fn handle_events(app: Arc<tokio::sync::Mutex<App>>) -> Result<()> {
    if event::poll(Duration::from_millis(100))? {
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                        let mut app_guard = app.lock().await;
                        app_guard.should_quit = true;
                    }
                    KeyCode::Char(' ') => {
                        let app_guard = app.lock().await;
                        app_guard.toggle_pause();
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Progress bar
            Constraint::Length(3), // Status
            Constraint::Length(3), // Controls
            Constraint::Min(0),    // Spacer
        ])
        .split(f.area());

    // Title
    let title = format!("Copying {} to {}", app.source_path, app.destination_path);
    let title_paragraph = Paragraph::new(title)
        .block(Block::default().borders(Borders::ALL).title("prcp - Progress Copy"));
    f.render_widget(title_paragraph, chunks[0]);

    // Progress bar
    let progress_ratio = if app.progress.total_bytes > 0 {
        app.progress.bytes_copied as f64 / app.progress.total_bytes as f64
    } else {
        0.0
    };

    let progress_bar = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(progress_ratio);
    f.render_widget(progress_bar, chunks[1]);

    // Status
    let status_text = if let Some(ref error) = app.progress.error {
        format!("Error: {}", error)
    } else if app.progress.is_finished {
        "Copy completed successfully!".to_string()
    } else {
        let throughput_str = format_throughput(app.progress.throughput);
        format!(
            "{} / {} - {} - {:.1}%",
            format_bytes(app.progress.bytes_copied),
            format_bytes(app.progress.total_bytes),
            throughput_str,
            progress_ratio * 100.0
        )
    };

    let status_paragraph = Paragraph::new(status_text)
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(status_paragraph, chunks[2]);

    // Controls
    let controls_text = if app.is_paused() {
        "Paused - Press SPACE to continue, CTRL+C to abort"
    } else {
        "Copying - Press SPACE to pause, CTRL+C to abort"
    };

    let controls_paragraph = Paragraph::new(controls_text)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(controls_paragraph, chunks[3]);
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1000.0 && unit_index < UNITS.len() - 1 {
        size /= 1000.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

fn format_throughput(bytes_per_second: u64) -> String {
    if bytes_per_second == 0 {
        "Unknown".to_string()
    } else {
        format!("{}/s", format_bytes(bytes_per_second))
    }
}