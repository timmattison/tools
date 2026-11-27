//! Tests for the terminal preview.

use super::*;
use crate::types::{Event, EventType};

fn secs(value: f64) -> Seconds {
    Seconds::new(value).unwrap()
}

fn event(time: f64, event_type: EventType, data: &str) -> Event {
    Event {
        time,
        event_type,
        data: data.to_string(),
    }
}

fn recording(width: u16, height: u16, events: Vec<Event>) -> Arc<Recording> {
    let duration = events.last().map_or(0.0, |last| last.time);
    Arc::new(Recording {
        version: 2,
        width,
        height,
        timestamp: 0.0,
        duration,
        command: String::new(),
        title: String::new(),
        env: Default::default(),
        events,
    })
}

fn first_row(state: &TerminalState) -> String {
    state.get_grid()[0].iter().map(|cell| cell.ch).collect()
}

#[test]
fn seek_replays_the_events_at_or_before_the_time() {
    let events = vec![
        event(0.0, EventType::Output, "a"),
        event(1.0, EventType::Output, "b"),
        event(1.5, EventType::Input, "q"),
        event(2.0, EventType::Output, "c"),
    ];
    let mut preview = Preview::new(recording(80, 24, events), PreviewTheme::Auto);

    assert!(first_row(preview.seek(secs(1.0))).starts_with("ab "));
    assert!(first_row(preview.seek(secs(0.5))).starts_with("a "));
    assert!(first_row(preview.seek(secs(1.7))).starts_with("ab "));
    assert!(!first_row(preview.seek(secs(1.7))).contains('q'));
    assert!(first_row(preview.seek(secs(2.0))).starts_with("abc "));
}
