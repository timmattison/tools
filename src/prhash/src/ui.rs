use ratatui::{
    prelude::*,
    widgets::*,
};
use crate::app::{App, AppState};

pub fn draw(f: &mut Frame, app: &App) {
    match &app.state {
        AppState::Error(error_msg) => {
            let error_paragraph = Paragraph::new(error_msg.as_str())
                .style(Style::default().fg(Color::Red));
            f.render_widget(error_paragraph, f.area());
        }
        AppState::Finished => {
            if let Some(hash_value) = &app.hash_result {
                let result_text = format!("{}  {}", hash_value, app.input_file.display());
                let result_paragraph = Paragraph::new(result_text)
                    .wrap(Wrap { trim: true });
                f.render_widget(result_paragraph, f.area());
            }
        }
        _ => {
            draw_main_ui(f, app);
        }
    }
}

fn draw_main_ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Length(3), // Progress bar
            Constraint::Length(3), // Stats
            Constraint::Length(3), // Controls
            Constraint::Min(0),    // Spacer
        ])
        .split(f.area());

    // Title
    let title_text = match &app.state {
        AppState::Preparing => "Waiting to start hashing...".to_string(),
        _ => {
            format!(
                "Hashing {} with {}",
                app.input_file.display(),
                app.hash_type
            )
        }
    };
    
    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: true });
    f.render_widget(title, chunks[0]);

    // Progress bar
    if !matches!(app.state, AppState::Preparing) {
        let progress = app.progress_percentage();
        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Magenta).bg(Color::Black))
            .percent(progress as u16)
            .label(format!("{:.1}%", progress));
        f.render_widget(gauge, chunks[1]);
    }

    // Stats
    if !matches!(app.state, AppState::Preparing) {
        let stats_text = format!(
            "[ {} / {} ]{}",
            app.format_bytes(app.bytes_processed),
            app.format_bytes(app.file_size),
            app.format_throughput()
        );
        
        let stats = Paragraph::new(stats_text)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true });
        f.render_widget(stats, chunks[2]);
    }

    // Controls
    let controls_text = match &app.state {
        AppState::Paused => "Paused  - press space to continue\nCTRL-C  - abort hash",
        AppState::Hashing => "Hashing - press space to pause\nCTRL-C  - abort hash",
        _ => "CTRL-C  - abort hash",
    };
    
    let controls = Paragraph::new(controls_text)
        .style(Style::default().fg(Color::Gray))
        .wrap(Wrap { trim: true });
    f.render_widget(controls, chunks[3]);
}