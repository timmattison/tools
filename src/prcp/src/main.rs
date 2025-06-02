use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use num_format::{Locale, ToFormattedString};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufReader, BufWriter, Read, Write, IsTerminal},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[cfg(test)]
mod tests;

#[derive(Parser)]
#[command(name = "prcp")]
#[command(about = "Progress copy tool with TUI")]
struct Args {
    /// Source file path
    source: PathBuf,
    /// Destination file path  
    destination: PathBuf,
}

#[derive(Debug)]
enum AppEvent {
    CopyProgress { bytes_copied: u64 },
    CopyComplete,
    CopyError(String),
    Tick,
}

struct App {
    source_path: PathBuf,
    destination_path: PathBuf,
    total_size: u64,
    bytes_copied: Arc<AtomicU64>,
    start_time: Instant,
    paused: Arc<AtomicBool>,
    should_quit: bool,
    error: Option<String>,
    copy_complete: bool,
}

impl App {
    fn new(source: PathBuf, destination: PathBuf) -> Result<Self> {
        let metadata = std::fs::metadata(&source)?;
        let total_size = metadata.len();
        
        Ok(Self {
            source_path: source,
            destination_path: destination,
            total_size,
            bytes_copied: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
            paused: Arc::new(AtomicBool::new(false)),
            should_quit: false,
            error: None,
            copy_complete: false,
        })
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::CopyProgress { bytes_copied } => {
                self.bytes_copied.store(bytes_copied, Ordering::Relaxed);
            }
            AppEvent::CopyComplete => {
                self.copy_complete = true;
            }
            AppEvent::CopyError(error) => {
                self.error = Some(error);
                self.should_quit = true;
            }
            AppEvent::Tick => {
                // Just update the display
            }
        }
    }

    fn toggle_pause(&self) {
        let current = self.paused.load(Ordering::Relaxed);
        self.paused.store(!current, Ordering::Relaxed);
    }

    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    fn get_progress(&self) -> f64 {
        if self.total_size == 0 {
            return 1.0;
        }
        let copied = self.bytes_copied.load(Ordering::Relaxed);
        copied as f64 / self.total_size as f64
    }

    fn get_throughput(&self) -> u64 {
        let elapsed = self.start_time.elapsed();
        let copied = self.bytes_copied.load(Ordering::Relaxed);
        if elapsed.as_secs() > 0 {
            copied / elapsed.as_secs()
        } else if elapsed.as_millis() > 0 {
            (copied * 1000) / elapsed.as_millis() as u64
        } else {
            copied // Very fast copy
        }
    }

    fn format_bytes(bytes: u64) -> String {
        const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
        let mut value = bytes as f64;
        let mut unit_index = 0;

        while value >= 1000.0 && unit_index < UNITS.len() - 1 {
            value /= 1000.0;
            unit_index += 1;
        }

        if unit_index == 0 {
            format!("{} {}", bytes.to_formatted_string(&Locale::en), UNITS[unit_index])
        } else {
            format!("{:.1} {}", value, UNITS[unit_index])
        }
    }
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(2)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Progress bar
            Constraint::Length(3), // Progress details
            Constraint::Length(3), // Status
            Constraint::Min(0),    // Instructions
        ])
        .split(f.area());

    // Title
    let title = Paragraph::new(vec![Line::from(vec![
        Span::raw("Copying "),
        Span::styled(
            app.source_path.display().to_string(),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" to "),
        Span::styled(
            app.destination_path.display().to_string(), 
            Style::default().fg(Color::Green),
        ),
    ])])
    .block(Block::default().borders(Borders::ALL).title("prcp"))
    .wrap(Wrap { trim: true });
    f.render_widget(title, chunks[0]);

    // Progress bar
    let progress = app.get_progress();
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(Color::Yellow))
        .percent((progress * 100.0) as u16)
        .label(format!("{:.1}%", progress * 100.0));
    f.render_widget(gauge, chunks[1]);

    // Progress details
    let copied = app.bytes_copied.load(Ordering::Relaxed);
    let throughput = app.get_throughput();
    let details = Paragraph::new(vec![Line::from(vec![
        Span::raw("[ "),
        Span::styled(
            App::format_bytes(copied),
            Style::default().fg(Color::White),
        ),
        Span::raw(" / "),
        Span::styled(
            App::format_bytes(app.total_size),
            Style::default().fg(Color::White),
        ),
        Span::raw(" ]   "),
        Span::styled(
            format!("{}/s", App::format_bytes(throughput)),
            Style::default().fg(Color::Green),
        ),
    ])])
    .block(Block::default().borders(Borders::ALL));
    f.render_widget(details, chunks[2]);

    // Status
    let status_text = if let Some(ref error) = app.error {
        format!("Error: {}", error)
    } else if app.copy_complete {
        "Copy complete!".to_string()
    } else if app.is_paused() {
        "Paused - press space to continue".to_string()
    } else {
        "Copying - press space to pause".to_string()
    };

    let status = Paragraph::new(status_text)
        .style(if app.error.is_some() {
            Style::default().fg(Color::Red)
        } else if app.copy_complete {
            Style::default().fg(Color::Green)
        } else if app.is_paused() {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        })
        .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(status, chunks[3]);

    // Instructions
    let instructions = Paragraph::new("SPACE: pause/resume | CTRL-C: quit")
        .style(Style::default().fg(Color::Gray))
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(instructions, chunks[4]);
}

async fn copy_file(
    source: PathBuf,
    destination: PathBuf,
    bytes_copied: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    tx: mpsc::UnboundedSender<AppEvent>,
) -> Result<()> {
    const BUFFER_SIZE: usize = 16 * 1024 * 1024; // 16MB buffer like Go version

    let mut source_file = BufReader::new(File::open(&source)?);
    let mut dest_file = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&destination)?,
    );

    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut total_copied = 0u64;

    loop {
        // Check for pause
        while paused.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        match source_file.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                dest_file.write_all(&buffer[..n])?;
                total_copied += n as u64;
                bytes_copied.store(total_copied, Ordering::Relaxed);
                
                let _ = tx.send(AppEvent::CopyProgress {
                    bytes_copied: total_copied,
                });
            }
            Err(e) => {
                let _ = tx.send(AppEvent::CopyError(e.to_string()));
                return Err(e.into());
            }
        }
    }

    dest_file.flush()?;
    let _ = tx.send(AppEvent::CopyComplete);
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Validate arguments
    if !args.source.exists() {
        anyhow::bail!("Source file does not exist: {}", args.source.display());
    }

    if args.source.is_dir() {
        anyhow::bail!("Source must be a file, not a directory");
    }

    // Try to set up terminal, fall back to simple copy if not available
    let use_tui = match (enable_raw_mode(), std::io::stdout().is_terminal()) {
        (Ok(_), true) => {
            // Try to set up the terminal
            match setup_terminal() {
                Ok(_) => true,
                Err(_) => {
                    let _ = disable_raw_mode();
                    false
                }
            }
        }
        _ => false,
    };

    if use_tui {
        run_with_tui(args).await
    } else {
        run_without_tui(args).await
    }
}

fn setup_terminal() -> Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(())
}

async fn run_without_tui(args: Args) -> Result<()> {
    println!("Copying {} to {}", args.source.display(), args.destination.display());
    
    let metadata = std::fs::metadata(&args.source)?;
    let total_size = metadata.len();
    
    let bytes_copied = Arc::new(AtomicU64::new(0));
    let paused = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::unbounded_channel();

    let start_time = Instant::now();
    
    // Start copy task
    let copy_handle = tokio::spawn(copy_file(
        args.source,
        args.destination,
        bytes_copied.clone(),
        paused.clone(),
        tx.clone(),
    ));

    // Simple progress reporting
    let progress_bytes = bytes_copied.clone();
    let _progress_handle = tokio::spawn(async move {
        let mut last_bytes = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let current_bytes = progress_bytes.load(Ordering::Relaxed);
            if current_bytes != last_bytes {
                let progress = if total_size > 0 {
                    (current_bytes as f64 / total_size as f64) * 100.0
                } else {
                    0.0
                };
                println!("Progress: {:.1}% ({} / {})", 
                    progress, 
                    App::format_bytes(current_bytes), 
                    App::format_bytes(total_size)
                );
                last_bytes = current_bytes;
            }
        }
    });

    // Wait for completion or error
    while let Some(event) = rx.recv().await {
        match event {
            AppEvent::CopyComplete => {
                let total_time = start_time.elapsed();
                let throughput = if total_time.as_secs() > 0 {
                    total_size / total_time.as_secs()
                } else if total_time.as_millis() > 0 {
                    (total_size * 1000) / total_time.as_millis() as u64
                } else {
                    total_size  // Very fast copy
                };
                println!("Copy completed successfully!");
                println!(
                    "Copied {} in {:.2} seconds ({}/s)",
                    App::format_bytes(total_size),
                    total_time.as_secs_f64(),
                    App::format_bytes(throughput)
                );
                break;
            }
            AppEvent::CopyError(error) => {
                eprintln!("Error: {}", error);
                std::process::exit(1);
            }
            _ => {}
        }
    }

    copy_handle.abort();
    Ok(())
}

async fn run_with_tui(args: Args) -> Result<()> {
    // Setup terminal
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(args.source.clone(), args.destination.clone())?;

    // Create channels for communication
    let (tx, mut rx) = mpsc::unbounded_channel();

    // Start copy task
    let copy_handle = tokio::spawn(copy_file(
        args.source,
        args.destination,
        app.bytes_copied.clone(),
        app.paused.clone(),
        tx.clone(),
    ));

    // Start tick task for UI updates
    let tick_tx = tx.clone();
    let _tick_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(100));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    // Main event loop
    let result = loop {
        // Handle terminal events
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('c') if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                            break Ok(());
                        }
                        KeyCode::Char(' ') => {
                            app.toggle_pause();
                        }
                        _ => {}
                    }
                }
            }
        }

        // Handle app events
        while let Ok(event) = rx.try_recv() {
            app.handle_event(event);
        }

        // Draw UI
        terminal.draw(|f| ui(f, &app))?;

        // Check if we should quit
        if app.should_quit || app.copy_complete {
            break Ok(());
        }
    };

    // Cleanup
    copy_handle.abort();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Show final result
    if let Some(error) = app.error {
        eprintln!("Error: {}", error);
        std::process::exit(1);
    } else if app.copy_complete {
        let total_time = app.start_time.elapsed();
        let throughput = app.get_throughput();
        println!("Copy completed successfully!");
        println!(
            "Copied {} in {:.2} seconds ({}/s)",
            App::format_bytes(app.total_size),
            total_time.as_secs_f64(),
            App::format_bytes(throughput)
        );
    }

    result
}