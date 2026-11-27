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

type Snapshot = (Vec<Vec<termstate::Cell>>, (usize, usize), bool);

fn capture(state: &TerminalState) -> Snapshot {
    (
        state.get_grid().clone(),
        state.get_cursor_position(),
        state.is_cursor_visible(),
    )
}

/// A recording with colors, a split escape sequence, new lines, and cursor
/// moves. It has more than 50 events.
fn busy_recording() -> Arc<Recording> {
    let mut events = Vec::new();
    for index in 0..60_u32 {
        let time = f64::from(index) * 0.1;
        let data = match index % 6 {
            0 => format!("line {index}\r\n"),
            1 => "\x1b[3".to_string(),
            2 => "1mred\x1b[0m".to_string(),
            3 => "\x1b[5;10Hmoved".to_string(),
            4 => "\x1b[?25l\x1b[44m blue \x1b[0m".to_string(),
            _ => "\x1b[?25h\x1b[1;32mgreen\x1b[0m\r\n".to_string(),
        };
        events.push(event(time, EventType::Output, &data));
        if index % 7 == 0 {
            events.push(event(time + 0.05, EventType::Input, "k"));
        }
    }
    recording(80, 24, events)
}

#[test]
fn any_order_of_seeks_gives_the_same_state_as_a_fresh_replay() {
    let recording = busy_recording();
    let times = [
        0.0, 2.5, 0.4, 0.6, 5.9, 1.0, 1.0, 3.3, 0.0, 4.05, 2.5, 100.0, 0.5, 5.99, 0.0,
    ];
    let mut long_lived = Preview::new(Arc::clone(&recording), PreviewTheme::Auto);
    for time in times {
        let got = capture(long_lived.seek(secs(time)));
        let mut fresh = Preview::new(Arc::clone(&recording), PreviewTheme::Auto);
        let want = capture(fresh.seek(secs(time)));
        assert_eq!(got, want, "seek to {time}");
    }
}

fn steady_recording(count: usize, step: f64, width: u16, height: u16) -> Arc<Recording> {
    let events = (0..count)
        .map(|index| event(index as f64 * step, EventType::Output, "x"))
        .collect();
    recording(width, height, events)
}

#[test]
fn snapshots_bound_the_replay_of_a_seek() {
    // 600 events, one each 0.1 s. The snapshot interval is 1 s.
    let mut preview = Preview::new(steady_recording(600, 0.1, 80, 24), PreviewTheme::Auto);
    preview.seek(secs(60.0));
    assert!(preview.snapshot_count() > 1);

    let before = preview.events_replayed();
    preview.seek(secs(30.05));
    assert!(preview.events_replayed() - before <= 11);

    let before = preview.events_replayed();
    preview.seek(secs(45.0));
    assert!(preview.events_replayed() - before <= 11);
}

#[test]
fn the_number_of_snapshots_has_a_limit() {
    let mut preview = Preview::new(steady_recording(100_000, 0.1, 80, 24), PreviewTheme::Auto);
    preview.seek(secs(10_000.0));
    assert!(preview.snapshot_count() > 1);
    assert!(preview.snapshot_count() <= 2 * 256 + 1);
}

#[test]
fn a_zero_size_recording_gives_a_one_cell_grid() {
    let events = vec![event(0.0, EventType::Output, "ab\r\ncd")];
    let mut preview = Preview::new(recording(0, 0, events), PreviewTheme::Auto);
    let grid = preview.seek(secs(1.0)).get_grid();
    assert_eq!(grid.len(), 1);
    assert_eq!(grid[0].len(), 1);
}

#[test]
fn a_huge_recording_size_is_clamped() {
    let events = vec![event(0.0, EventType::Output, "ab\r\ncd")];
    let mut preview = Preview::new(recording(60_000, 60_000, events), PreviewTheme::Auto);
    let grid = preview.seek(secs(1.0)).get_grid();
    assert_eq!(grid.len(), 500);
    assert_eq!(grid[0].len(), 500);
}

#[test]
fn the_theme_list_has_the_five_themes_in_order() {
    assert_eq!(
        PreviewTheme::ALL,
        [
            PreviewTheme::Auto,
            PreviewTheme::Dracula,
            PreviewTheme::Monokai,
            PreviewTheme::SolarizedDark,
            PreviewTheme::SolarizedLight,
        ]
    );
    assert_eq!(PreviewTheme::default(), PreviewTheme::Auto);
}

#[test]
fn a_theme_name_round_trips_and_a_wrong_name_is_refused() {
    for theme in PreviewTheme::ALL {
        assert_eq!(PreviewTheme::from_name(theme.name()), Some(theme));
    }
    assert_eq!(PreviewTheme::from_name("Dracula"), None);
    assert_eq!(PreviewTheme::from_name("black"), None);
    assert_eq!(PreviewTheme::from_name(""), None);
}

#[test]
fn the_names_and_labels_are_the_beta_values() {
    let names: Vec<_> = PreviewTheme::ALL.iter().map(|t| t.name()).collect();
    assert_eq!(
        names,
        [
            "auto",
            "dracula",
            "monokai",
            "solarized-dark",
            "solarized-light"
        ]
    );
    let labels: Vec<_> = PreviewTheme::ALL.iter().map(|t| t.label()).collect();
    assert_eq!(
        labels,
        [
            "Auto",
            "Dracula",
            "Monokai",
            "Solarized Dark",
            "Solarized Light"
        ]
    );
}

#[test]
fn the_palette_matches_the_constructor_of_the_theme() {
    let want = [
        TerminalTheme::auto(),
        TerminalTheme::dracula(),
        TerminalTheme::monokai(),
        TerminalTheme::solarized_dark(),
        TerminalTheme::solarized_light(),
    ];
    for (theme, want) in PreviewTheme::ALL.into_iter().zip(want) {
        assert_eq!(theme.palette().background, want.background);
        assert_eq!(theme.palette().foreground, want.foreground);
    }
    assert_ne!(
        PreviewTheme::Dracula.palette().background,
        PreviewTheme::Auto.palette().background
    );
}

#[test]
fn a_preview_draws_blank_cells_with_the_background_of_its_theme() {
    let mut preview = Preview::new(recording(80, 24, Vec::new()), PreviewTheme::Dracula);
    assert_eq!(preview.theme(), PreviewTheme::Dracula);
    let state = preview.seek(secs(0.0));
    assert_eq!(state.get_grid()[0][0].bg_color, (40, 42, 54));
}

#[test]
fn the_snapshots_stay_in_the_cell_budget() {
    // 30,000 dense events fill the event axis. 100 sparse events fill the
    // time axis. Together they reach the worst case of `2 * budget + 1`.
    let mut events: Vec<Event> = (0..30_000)
        .map(|i| event(f64::from(i) / 30_000.0, EventType::Output, "x"))
        .collect();
    events.extend((0..100).map(|i| event(1.0 + f64::from(i), EventType::Output, "x")));
    let mut preview = Preview::new(recording(500, 500, events), PreviewTheme::Auto);
    preview.seek(secs(1000.0));

    let cells = 500 * 500;
    let count = preview.snapshot_count();
    assert!(
        count * cells <= SNAPSHOT_CELL_BUDGET,
        "{count} snapshots of {cells} cells go past the budget"
    );
    assert!(
        count * cells > SNAPSHOT_CELL_BUDGET / 2,
        "{count} snapshots do not fill the budget"
    );
}
