use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event as TermEvent, KeyCode},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::fs::File;
use std::io::{stdout, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{interval, Instant};
use tokio_stream::StreamExt;

use crate::{EventType, Recording};

pub async fn play(file_path: PathBuf, speed: f64, paused: bool) -> Result<()> {
    let file = File::open(&file_path)
        .context("Failed to open recording file")?;
    
    let recording: Recording = if file.metadata()?.len() > 0 {
        let reader = BufReader::new(file);
        
        if file_path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with(".gz"))
            .unwrap_or(false)
        {
            let decoder = flate2::read::GzDecoder::new(reader);
            serde_json::from_reader(decoder)
                .context("Failed to parse compressed recording")?
        } else {
            serde_json::from_reader(reader)
                .context("Failed to parse recording")?
        }
    } else {
        anyhow::bail!("Recording file is empty");
    };
    
    println!("Playing recording: {}", recording.title);
    println!("Duration: {:.1}s", recording.duration);
    println!("Dimensions: {}x{}", recording.width, recording.height);
    println!("Events: {}", recording.events.len());
    println!();
    println!("Controls:");
    println!("  Space: Pause/Resume");
    println!("  ←/→: Rewind/Fast-forward 5s");
    println!("  ↑/↓: Speed up/down");
    println!("  q: Quit");
    println!();
    println!("Press any key to start...");
    
    terminal::enable_raw_mode()?;
    event::read()?;
    
    execute!(
        stdout(),
        EnterAlternateScreen,
        Clear(ClearType::All),
        MoveTo(0, 0),
        Hide
    )?;
    
    let mut current_event_idx = 0;
    let mut is_paused = paused;
    let mut playback_speed = speed;
    let mut elapsed_time = 0.0;
    let start_instant = Instant::now();
    
    let mut event_stream = tokio_stream::StreamExt::fuse(
        crossterm::event::EventStream::new()
    );
    
    let mut ticker = interval(Duration::from_millis(16)); // ~60 FPS
    
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if !is_paused && current_event_idx < recording.events.len() {
                    let current_time = if playback_speed == 1.0 {
                        start_instant.elapsed().as_secs_f64() + elapsed_time
                    } else {
                        elapsed_time + (0.016 * playback_speed)
                    };
                    
                    while current_event_idx < recording.events.len() {
                        let event = &recording.events[current_event_idx];
                        if event.time <= current_time {
                            match event.event_type {
                                EventType::Output => {
                                    print!("{}", event.data);
                                    stdout().flush()?;
                                }
                                EventType::Input => {
                                    // Optionally show input in a different style
                                }
                            }
                            current_event_idx += 1;
                        } else {
                            break;
                        }
                    }
                    
                    if playback_speed != 1.0 {
                        elapsed_time = current_time;
                    }
                    
                    if current_event_idx >= recording.events.len() {
                        break;
                    }
                }
            }
            
            Some(Ok(event)) = event_stream.next() => {
                if let TermEvent::Key(key) = event {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char(' ') => {
                            is_paused = !is_paused;
                            if !is_paused {
                                // Reset timing when resuming
                                elapsed_time = recording.events[current_event_idx.min(recording.events.len() - 1)].time;
                            }
                        }
                        KeyCode::Left => {
                            // Rewind 5 seconds
                            let target_time = (elapsed_time - 5.0).max(0.0);
                            rewind_to_time(&recording, target_time, &mut current_event_idx)?;
                            elapsed_time = target_time;
                        }
                        KeyCode::Right => {
                            // Fast-forward 5 seconds
                            let target_time = (elapsed_time + 5.0).min(recording.duration);
                            fast_forward_to_time(&recording, target_time, &mut current_event_idx)?;
                            elapsed_time = target_time;
                        }
                        KeyCode::Up => {
                            playback_speed = (playback_speed * 1.5).min(10.0);
                        }
                        KeyCode::Down => {
                            playback_speed = (playback_speed / 1.5).max(0.1);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    
    execute!(
        stdout(),
        Show,
        LeaveAlternateScreen
    )?;
    terminal::disable_raw_mode()?;
    
    if current_event_idx >= recording.events.len() {
        println!("\nPlayback complete!");
    } else {
        println!("\nPlayback stopped at {:.1}s", elapsed_time);
    }
    
    Ok(())
}

fn rewind_to_time(recording: &Recording, target_time: f64, current_idx: &mut usize) -> Result<()> {
    execute!(stdout(), Clear(ClearType::All), MoveTo(0, 0))?;
    
    *current_idx = 0;
    for (idx, event) in recording.events.iter().enumerate() {
        if event.time > target_time {
            break;
        }
        if matches!(event.event_type, EventType::Output) {
            print!("{}", event.data);
        }
        *current_idx = idx + 1;
    }
    stdout().flush()?;
    Ok(())
}

fn fast_forward_to_time(recording: &Recording, target_time: f64, current_idx: &mut usize) -> Result<()> {
    while *current_idx < recording.events.len() {
        let event = &recording.events[*current_idx];
        if event.time > target_time {
            break;
        }
        if matches!(event.event_type, EventType::Output) {
            print!("{}", event.data);
            stdout().flush()?;
        }
        *current_idx += 1;
    }
    Ok(())
}