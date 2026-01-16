use buildinfo::version_string;
use chrono::{DateTime, Datelike, Local, NaiveDateTime, TimeZone};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use std::{
    error::Error,
    io,
    process::{exit, Command},
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(name = "runat")]
#[command(version = version_string!())]
#[command(about = "Run a command at a specified time")]
struct Cli {
    /// Target time in various formats (RFC3339, YYYY-MM-DD HH:MM:SS, HH:MM, etc.)
    time: String,
    
    /// Command to run
    command: Vec<String>,
}

struct App {
    target_time: DateTime<Local>,
    command: Vec<String>,
    should_quit: bool,
}

impl App {
    fn new(target_time: DateTime<Local>, command: Vec<String>) -> App {
        App {
            target_time,
            command,
            should_quit: false,
        }
    }
    
    fn should_execute(&self) -> bool {
        Local::now() >= self.target_time
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    
    if cli.command.is_empty() {
        eprintln!("Usage: runat <timestamp> <command> [args...]");
        eprintln!("Examples:");
        eprintln!("  runat 2024-01-01T12:00:00Z echo hello world    # UTC time");
        eprintln!("  runat 2024-01-01T12:00:00 echo hello world     # Local time");
        eprintln!("  runat \"2024-01-01 12:00\" echo hello world      # Local time");
        eprintln!("  runat 12:00 echo hello world                   # Today/tomorrow at 12:00 local time");
        exit(1);
    }
    
    let target_time = match parse_time_string(&cli.time) {
        Ok(time) => time,
        Err(e) => {
            eprintln!("Invalid timestamp format: {}", e);
            exit(1);
        }
    };
    
    if target_time <= Local::now() {
        eprintln!("Target time must be in the future");
        exit(1);
    }
    
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    
    // Clear the screen before displaying anything
    terminal.clear()?;
    
    // Create app and run
    let app = App::new(target_time, cli.command);
    let command_to_run = run_app(&mut terminal, app)?;
    
    // Always restore terminal before doing anything else
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    
    // Now execute the command if we have one
    if let Some(command) = command_to_run {
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..]);
        
        match cmd.status() {
            Ok(status) => {
                exit(status.code().unwrap_or(0));
            }
            Err(e) => {
                eprintln!("Failed to run command: {}", e);
                exit(1);
            }
        }
    }
    
    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<Option<Vec<String>>> {
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(1000);
    
    loop {
        terminal.draw(|f| ui(f, &app))?;
        
        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
            
        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('c') => {
                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) {
                            app.should_quit = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        
        if last_tick.elapsed() >= tick_rate {
            last_tick = Instant::now();
            
            if app.should_execute() {
                // Exit TUI mode before running command
                break;
            }
        }
        
        if app.should_quit {
            return Ok(None);
        }
    }
    
    // Return the command to be executed
    Ok(Some(app.command))
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(f.size());
    
    let now = Local::now();
    let remaining = app.target_time.signed_duration_since(now);
    
    let hours = remaining.num_hours();
    let minutes = remaining.num_minutes() % 60;
    let seconds = remaining.num_seconds() % 60;
    
    // Current time
    let current_time = Paragraph::new(format!("Current time: {}", format_time(now)))
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    f.render_widget(current_time, chunks[0]);
    
    // Target time
    let target_time = Paragraph::new(format!("Target time:  {}", format_time(app.target_time)))
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default());
    f.render_widget(target_time, chunks[1]);
    
    // Time remaining
    let remaining_text = if remaining.num_seconds() > 0 {
        format!("Time remaining: {:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        "Time remaining: 00:00:00".to_string()
    };
    
    let time_remaining = Paragraph::new(remaining_text)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
    f.render_widget(time_remaining, chunks[2]);
    
    // Command
    let command_text = format!("Command: {}", app.command.join(" "));
    let command = Paragraph::new(command_text)
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(command, chunks[3]);
    
    // Instructions
    let instructions = Paragraph::new("Press CTRL-C to abort")
        .block(Block::default().borders(Borders::NONE))
        .style(Style::default())
        .alignment(Alignment::Center);
    f.render_widget(instructions, chunks[4]);
}

fn format_time(time: DateTime<Local>) -> String {
    time.format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

fn parse_time_string(time_str: &str) -> Result<DateTime<Local>, Box<dyn Error>> {
    // Try parsing as RFC3339 (with timezone)
    if let Ok(dt) = DateTime::parse_from_rfc3339(time_str) {
        return Ok(dt.with_timezone(&Local));
    }
    
    // Try parsing common formats without timezone (assume local)
    let formats = vec![
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%H:%M:%S",
        "%H:%M",
    ];
    
    let now = Local::now();
    let today = Local.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0).single().unwrap();
    
    for format in &formats {
        if let Ok(naive_dt) = NaiveDateTime::parse_from_str(time_str, format) {
            let dt = Local.from_local_datetime(&naive_dt).single()
                .ok_or("Invalid local time")?;
            return Ok(dt);
        }
        
        // For time-only formats, try parsing just the time
        if format == &"%H:%M:%S" || format == &"%H:%M" {
            if let Ok(naive_time) = chrono::NaiveTime::parse_from_str(time_str, format) {
                // Create datetime in local timezone directly
                let today_at_time = today.date_naive().and_time(naive_time);
                let dt_today = Local.from_local_datetime(&today_at_time).single()
                    .ok_or("Invalid local time")?;
                
                // Calculate both today and tomorrow options
                let dt_tomorrow = dt_today + chrono::Duration::days(1);
                
                // Choose the closest future time
                let chosen_dt = if dt_today > now {
                    // Time hasn't passed today, use it
                    dt_today
                } else {
                    // Time has passed today, use tomorrow
                    dt_tomorrow
                };
                
                // For 12-hour ambiguity: if the chosen time is more than 12 hours away,
                // check if the opposite AM/PM would be closer and still in the future
                let duration_to_chosen = chosen_dt.signed_duration_since(now);
                if duration_to_chosen.num_hours() > 12 {
                    // Try the opposite AM/PM (subtract 12 hours)
                    let alternative = chosen_dt - chrono::Duration::hours(12);
                    if alternative > now {
                        // The alternative is in the future and closer, use it
                        return Ok(alternative);
                    }
                }
                
                return Ok(chosen_dt);
            }
        }
    }
    
    Err(format!("Could not parse time: {}", time_str).into())
}