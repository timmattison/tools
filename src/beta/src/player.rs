use anyhow::Result;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event as TermEvent, KeyCode},
    execute,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::{interval, Instant};
use tokio_stream::StreamExt;

use crate::{EventType, Recording};

/// RAII guard to restore terminal state on drop (including on error/panic).
struct PlayerTerminalGuard;

impl Drop for PlayerTerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

pub async fn play(file_path: PathBuf, speed: f64, paused: bool) -> Result<()> {
    let recording = Recording::load(&file_path)?;

    println!("Playing recording: {}", recording.title);
    println!("Duration: {:.1}s", recording.duration);
    println!("Dimensions: {}x{}", recording.width, recording.height);
    println!("Events: {}", recording.events.len());
    println!();
    println!("Controls:");
    println!("  Space: Pause/Resume");
    println!("  \u{2190}/\u{2192}: Rewind/Fast-forward 5s");
    println!("  \u{2191}/\u{2193}: Speed up/down");
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

    // Guard ensures terminal is restored even if we return early via `?`
    let _guard = PlayerTerminalGuard;

    let mut current_event_idx = 0;
    let mut is_paused = paused;
    let mut playback_speed = speed;

    // Unified timing: track virtual playback position and advance by real delta * speed
    let mut virtual_time = 0.0;
    let mut last_tick = Instant::now();

    let mut event_stream = crossterm::event::EventStream::new().fuse();

    let mut ticker = interval(Duration::from_millis(16)); // ~60 FPS

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if !is_paused && current_event_idx < recording.events.len() {
                    let now = Instant::now();
                    let real_delta = now.duration_since(last_tick).as_secs_f64();
                    last_tick = now;
                    virtual_time += real_delta * playback_speed;

                    while current_event_idx < recording.events.len() {
                        let event = &recording.events[current_event_idx];
                        if event.time <= virtual_time {
                            if matches!(event.event_type, EventType::Output) {
                                print!("{}", event.data);
                                stdout().flush()?;
                            }
                            current_event_idx += 1;
                        } else {
                            break;
                        }
                    }

                    if current_event_idx >= recording.events.len() {
                        break;
                    }
                } else {
                    // Keep last_tick current while paused so we don't get a huge delta on resume
                    last_tick = Instant::now();
                }
            }

            Some(Ok(event)) = event_stream.next() => {
                if let TermEvent::Key(key) = event {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char(' ') => {
                            is_paused = !is_paused;
                            last_tick = Instant::now();
                        }
                        KeyCode::Left => {
                            // Rewind 5 seconds
                            let target_time = (virtual_time - 5.0).max(0.0);
                            rewind_to_time(&recording, target_time, &mut current_event_idx)?;
                            virtual_time = target_time;
                            last_tick = Instant::now();
                        }
                        KeyCode::Right => {
                            // Fast-forward 5 seconds
                            let target_time = (virtual_time + 5.0).min(recording.duration);
                            fast_forward_to_time(&recording, target_time, &mut current_event_idx)?;
                            virtual_time = target_time;
                            last_tick = Instant::now();
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

    // Guard handles cleanup on drop, but we print status after it
    drop(_guard);

    if current_event_idx >= recording.events.len() {
        println!("\nPlayback complete!");
    } else {
        println!("\nPlayback stopped at {:.1}s", virtual_time);
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
