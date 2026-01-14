use anyhow::Result;
use chrono::{DateTime, Local};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use parking_lot::Mutex;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

/// Progress information for the UI
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    pub dirs_checked: usize,
    pub repos_found: usize,
    pub current_path: String,
    pub start_time: Instant,
    pub threshold_time: DateTime<Local>,
    pub end_time: Option<DateTime<Local>>,
}

/// Simple progress display for the terminal UI
pub struct ProgressDisplay {
    dirs_checked: Arc<AtomicUsize>,
    repos_found: Arc<AtomicUsize>,
    current_path: Arc<Mutex<String>>,
    start_time: Instant,
    threshold_time: DateTime<Local>,
    end_time: Option<DateTime<Local>>,
    cancelled: Arc<AtomicBool>,
    scan_complete: Arc<AtomicBool>,
    // Ollama-related fields
    ollama_active: Arc<AtomicBool>,
    ollama_status: Arc<Mutex<String>>,
    ollama_repo: Arc<Mutex<String>>,
    ollama_progress: Arc<Mutex<String>>,
    ollama_complete: Arc<AtomicBool>,
    // Scan completion tracking
    scan_completion_time: Arc<Mutex<Option<Instant>>>,
    // Cancellation support
    cancellation_token: CancellationToken,
}

impl ProgressDisplay {
    pub fn new(threshold_time: DateTime<Local>, end_time: Option<DateTime<Local>>, ollama_enabled: bool) -> Self {
        Self {
            dirs_checked: Arc::new(AtomicUsize::new(0)),
            repos_found: Arc::new(AtomicUsize::new(0)),
            current_path: Arc::new(Mutex::new(String::new())),
            start_time: Instant::now(),
            threshold_time,
            end_time,
            cancelled: Arc::new(AtomicBool::new(false)),
            scan_complete: Arc::new(AtomicBool::new(false)),
            // Initialize Ollama fields
            ollama_active: Arc::new(AtomicBool::new(ollama_enabled)),
            ollama_status: Arc::new(Mutex::new("Waiting for scan to complete...".to_string())),
            ollama_repo: Arc::new(Mutex::new(String::new())),
            ollama_progress: Arc::new(Mutex::new(String::new())),
            ollama_complete: Arc::new(AtomicBool::new(false)),
            // Initialize scan completion tracking
            scan_completion_time: Arc::new(Mutex::new(None)),
            // Initialize cancellation token
            cancellation_token: CancellationToken::new(),
        }
    }

    pub fn update_progress(&self, dirs_checked: usize, repos_found: usize, current_path: String) {
        self.dirs_checked.store(dirs_checked, Ordering::Relaxed);
        self.repos_found.store(repos_found, Ordering::Relaxed);
        *self.current_path.lock() = current_path;
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub fn set_scan_complete(&self) {
        self.scan_complete.store(true, Ordering::Relaxed);
        // Capture the completion time
        *self.scan_completion_time.lock() = Some(Instant::now());
    }

    pub fn is_scan_complete(&self) -> bool {
        self.scan_complete.load(Ordering::Relaxed)
    }

    // Ollama status methods
    #[allow(dead_code)]
    pub fn set_ollama_active(&self, active: bool) {
        self.ollama_active.store(active, Ordering::Relaxed);
    }

    pub fn is_ollama_active(&self) -> bool {
        self.ollama_active.load(Ordering::Relaxed)
    }

    pub fn update_ollama_status(&self, status: String) {
        *self.ollama_status.lock() = status;
    }

    pub fn update_ollama_repo(&self, repo: String) {
        *self.ollama_repo.lock() = repo;
    }

    pub fn update_ollama_progress(&self, progress: String) {
        *self.ollama_progress.lock() = progress;
    }

    pub fn set_ollama_complete(&self) {
        self.ollama_complete.store(true, Ordering::Relaxed);
    }

    pub fn is_ollama_complete(&self) -> bool {
        self.ollama_complete.load(Ordering::Relaxed)
    }

    pub fn is_all_complete(&self) -> bool {
        self.is_scan_complete() && (!self.is_ollama_active() || self.is_ollama_complete())
    }

    pub fn should_show_ollama_panel(&self) -> bool {
        self.is_ollama_active()
    }

    #[allow(dead_code)]
    pub fn should_exit_ui(&self) -> bool {
        self.is_scan_complete()
    }

    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Run the interactive terminal UI.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup fails or if the UI loop encounters an error.
    pub fn run_interactive(&self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.run_ui_loop(&mut terminal);

        // Cleanup
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    }

    fn run_ui_loop<B: ratatui::backend::Backend>(&self, terminal: &mut Terminal<B>) -> Result<()> {
        let mut completion_shown = false;
        let mut completion_time: Option<Instant> = None;
        
        loop {
            // Check if all processing is complete (scanning + Ollama if enabled)
            if self.is_all_complete() && !completion_shown {
                completion_shown = true;
                completion_time = Some(Instant::now());
            }
            
            // Exit after showing completion for 1 second
            if let Some(time) = completion_time {
                if time.elapsed() > Duration::from_secs(1) {
                    break;
                }
            }
            
            terminal.draw(|f| {
                let show_ollama = self.should_show_ollama_panel();
                let constraints = if show_ollama {
                    vec![
                        Constraint::Length(3),  // Title
                        Constraint::Length(8),  // Stats
                        Constraint::Length(3),  // Current Path
                        Constraint::Min(4),     // Ollama Status - expands to use available space
                        Constraint::Length(3),  // Instructions
                    ]
                } else {
                    vec![
                        Constraint::Length(3),  // Title
                        Constraint::Length(8),  // Stats
                        Constraint::Length(3),  // Current Path
                        Constraint::Length(3),  // Instructions
                        Constraint::Min(0),     // Remaining space
                    ]
                };
                
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(constraints)
                    .split(f.area());

                // Title
                let title_text = if completion_shown {
                    "✅ Scan complete! Processing results...".to_string()
                } else if let Some(end) = self.end_time {
                    format!(
                        "🔍 Searching for commits between {} and {}",
                        self.threshold_time.format("%A, %B %d, %Y at %l:%M %p"),
                        end.format("%A, %B %d, %Y at %l:%M %p")
                    )
                } else {
                    format!(
                        "🔍 Searching for commits since {}",
                        self.threshold_time.format("%A, %B %d, %Y at %l:%M %p")
                    )
                };

                let title = Paragraph::new(title_text)
                    .block(Block::default().borders(Borders::ALL))
                    .style(Style::default().fg(Color::Cyan));
                f.render_widget(title, chunks[0]);

                // Stats
                let dirs_checked = self.dirs_checked.load(Ordering::Relaxed);
                let repos_found = self.repos_found.load(Ordering::Relaxed);
                
                // Use frozen time if scan is complete, otherwise current elapsed time
                let elapsed = if self.is_scan_complete() {
                    if let Some(completion_time) = *self.scan_completion_time.lock() {
                        completion_time.duration_since(self.start_time)
                    } else {
                        self.start_time.elapsed()
                    }
                } else {
                    self.start_time.elapsed()
                };
                
                let scan_rate = if self.is_scan_complete() {
                    // Don't show scan rate after completion
                    -1.0  // Sentinel value to indicate completion
                } else if elapsed.as_secs() > 0 {
                    dirs_checked as f64 / elapsed.as_secs() as f64
                } else {
                    0.0
                };

                let scan_complete = self.is_scan_complete();
                let mut stats_lines = vec![
                    Line::from(vec![
                        Span::raw("Directories scanned: "),
                        Span::styled(
                            if scan_complete { format!("{} ✓", dirs_checked) } else { dirs_checked.to_string() },
                            Style::default().fg(Color::Yellow)
                        ),
                    ]),
                    Line::from(vec![
                        Span::raw("Repositories found: "),
                        Span::styled(
                            if scan_complete { format!("{} ✓", repos_found) } else { repos_found.to_string() },
                            Style::default().fg(Color::Green)
                        ),
                    ]),
                    Line::from(vec![
                        Span::raw(if scan_complete { "Scan duration: " } else { "Time elapsed: " }),
                        Span::styled(format!("{:?}", elapsed), Style::default().fg(Color::Blue)),
                    ]),
                ];
                
                // Add scan rate or status line
                if scan_rate >= 0.0 {
                    stats_lines.push(Line::from(vec![
                        Span::raw("Scan rate: "),
                        Span::styled(format!("{:.1} dirs/sec", scan_rate), Style::default().fg(Color::Magenta)),
                    ]));
                } else {
                    stats_lines.push(Line::from(vec![
                        Span::raw("Status: "),
                        Span::styled(
                            if self.is_ollama_active() { "Processing with Ollama..." } else { "Scan complete" },
                            Style::default().fg(Color::Cyan)
                        ),
                    ]));
                }
                
                let stats_text = stats_lines;

                let stats = Paragraph::new(stats_text)
                    .block(Block::default().borders(Borders::ALL).title("Statistics"));
                f.render_widget(stats, chunks[1]);

                // Current path
                let current_text = if completion_shown {
                    "✅ Scanning complete - preparing results...".to_string()
                } else {
                    let current_path = self.current_path.lock().clone();
                    // Extract repo name from path if it contains .git
                    let display_path = if current_path.contains(".git") {
                        // Get the parent directory of .git (the actual repo)
                        if let Some(repo_end) = current_path.rfind(".git") {
                            current_path[..repo_end].trim_end_matches('/').to_string()
                        } else {
                            current_path.clone()
                        }
                    } else {
                        current_path.clone()
                    };
                    
                    let truncated_path = if display_path.len() > 60 {
                        format!("...{}", &display_path[display_path.len().saturating_sub(57)..])
                    } else {
                        display_path
                    };
                    format!("🔎 Current: {}", truncated_path)
                };

                let current = Paragraph::new(current_text)
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(current, chunks[2]);

                // Conditionally render Ollama Status Panel
                let instructions_index = if show_ollama {
                    let status = self.ollama_status.lock().clone();
                    let repo = self.ollama_repo.lock().clone();
                    let progress = self.ollama_progress.lock().clone();
                    
                    let ollama_text = if self.is_ollama_complete() {
                        "✅ Ollama processing complete".to_string()
                    } else if !repo.is_empty() {
                        format!("🤖 Processing: {}\n{}", repo, progress)
                    } else {
                        status
                    };

                    let ollama_color = if self.is_ollama_complete() {
                        Color::Green
                    } else {
                        Color::Yellow
                    };

                    let ollama_panel = Paragraph::new(ollama_text)
                        .block(Block::default().borders(Borders::ALL).title("Ollama Status"))
                        .style(Style::default().fg(ollama_color));
                    f.render_widget(ollama_panel, chunks[3]);
                    
                    4  // Instructions will be at index 4 when Ollama panel is shown
                } else {
                    3  // Instructions will be at index 3 when Ollama panel is hidden
                };

                // Instructions
                let instructions = Paragraph::new("Press 'q', 'Esc', or 'Ctrl+C' to quit")
                    .block(Block::default().borders(Borders::ALL).title("Instructions"))
                    .style(Style::default().fg(Color::Gray));
                f.render_widget(instructions, chunks[instructions_index]);
            })?;

            // Handle input
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc | KeyCode::Char('c') => {
                                self.cancelled.store(true, Ordering::Relaxed);
                                self.cancellation_token.cancel();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Simple non-interactive progress display for when TUI is not desired
    ///
    /// # Panics
    ///
    /// Panics if stdout flush fails.
    pub fn print_simple_progress(&self) {
        let dirs_checked = self.dirs_checked.load(Ordering::Relaxed);
        let repos_found = self.repos_found.load(Ordering::Relaxed);
        let elapsed = self.start_time.elapsed();
        
        if self.is_all_complete() {
            println!("\r✅ All processing complete! Scanned: {} dirs, Found: {} repos", dirs_checked, repos_found);
            return;
        }
        
        let scan_rate = if elapsed.as_secs() > 0 {
            dirs_checked as f64 / elapsed.as_secs() as f64
        } else {
            0.0
        };

        if self.is_ollama_active() && self.is_scan_complete() {
            let ollama_repo = self.ollama_repo.lock().clone();
            let ollama_progress = self.ollama_progress.lock().clone();
            
            print!(
                "\r🤖 Ollama: Processing {}, {} | Scanned: {} dirs, Found: {} repos",
                ollama_repo,
                ollama_progress,
                dirs_checked,
                repos_found
            );
        } else {
            let current_path = self.current_path.lock().clone();
            
            // Extract repo name from path if it contains .git
            let display_path = if current_path.contains(".git") {
                // Get the parent directory of .git (the actual repo)
                if let Some(repo_end) = current_path.rfind(".git") {
                    current_path[..repo_end].trim_end_matches('/').to_string()
                } else {
                    current_path.clone()
                }
            } else {
                current_path.clone()
            };
            
            print!(
                "\r🔍 Scanned: {} dirs, Found: {} repos, Rate: {:.1} dirs/sec, Current: {}",
                dirs_checked,
                repos_found,
                scan_rate,
                if display_path.len() > 40 {
                    format!("...{}", &display_path[display_path.len().saturating_sub(37)..])
                } else {
                    display_path
                }
            );
        }
        
        use std::io::Write;
        io::stdout().flush().unwrap();
    }
}

// We need parking_lot for a simpler Mutex API
// Add to Cargo.toml: parking_lot = "0.12"