//! Behavior tests for `chf::render`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use chf::app::{App, Drawn, Effect};
use chf::index::IndexEvent;
use chf::render::{draw, Context};
use chf::search::run_job;
use chf::session::{CompactionPoint, MessageSpan, Role, Session};
use chrono::{TimeZone, Utc};
use claude_transcript::{CompactionFacts, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

const ID_A: &str = "00000000-0000-4000-8000-00000000000a";
const ID_B: &str = "00000000-0000-4000-8000-00000000000b";
const ID_C: &str = "00000000-0000-4000-8000-00000000000c";
const HOME: &str = "/home/u";

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn shift_up() -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::SHIFT,
        ..press(KeyCode::Up)
    }
}

/// A session with the given message texts. `hour` sets `last_activity`.
fn session(id: &str, texts: &[&str], hour: u32) -> Session {
    let mut text = String::new();
    let mut messages = Vec::new();
    for t in texts {
        let start = text.len();
        text.push_str(t);
        messages.push(MessageSpan {
            role: Role::User,
            start,
            end: text.len(),
        });
    }
    Session {
        id: SessionId::parse(id).unwrap(),
        path: PathBuf::from("/x/session.jsonl"),
        cwd: Some("/home/u/code/x".to_string()),
        git_branch: Some("main".to_string()),
        ai_title: Some("Title".to_string()),
        last_activity: Utc.with_ymd_and_hms(2026, 11, 1, hour, 0, 0).unwrap(),
        compaction: CompactionFacts {
            compacted: false,
            compactions: 0,
            last_pre_tokens: None,
        },
        text,
        messages,
        compaction_points: Vec::new(),
    }
}

/// A compacted session with 2 compactions and a compaction point before
/// message `before`.
fn compacted(id: &str, texts: &[&str], before: usize) -> Session {
    let mut s = session(id, texts, 10);
    s.compaction = CompactionFacts {
        compacted: true,
        compactions: 2,
        last_pre_tokens: Some(259_822),
    };
    s.compaction_points = vec![CompactionPoint {
        before_message: before,
        pre_tokens: Some(259_822),
    }];
    s
}

fn settle(app: &mut App, effects: Vec<Effect>) {
    for effect in effects {
        if let Effect::Search(job) = effect {
            let hits = run_job(&job, &AtomicU64::new(job.generation)).unwrap();
            app.on_hits(hits);
        }
    }
}

fn app_with(sessions: Vec<Session>) -> App {
    let mut app = App::new();
    let events = sessions
        .into_iter()
        .map(|s| IndexEvent::Session(Box::new(s)))
        .collect();
    let effects = app.on_index(events);
    settle(&mut app, effects);
    app
}

fn three() -> App {
    app_with(vec![
        session(ID_A, &["alpha one"], 10),
        session(ID_B, &["beta one"], 11),
        session(ID_C, &["gamma two"], 12),
    ])
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        let effects = app.on_key(press(KeyCode::Char(c)));
        settle(app, effects);
    }
}

fn lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut line = String::new();
            let mut skip = 0_usize;
            for x in 0..area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let symbol = buffer[(x, y)].symbol();
                skip = unicode_width::UnicodeWidthStr::width(symbol).saturating_sub(1);
                line.push_str(symbol);
            }
            line.trim_end().to_owned()
        })
        .collect()
}

fn render_to(app: &App, width: u16, height: u16) -> (Buffer, Drawn) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let ctx = Context {
        home: Some(Path::new(HOME)),
        tz: &Utc,
    };
    let mut drawn = None;
    terminal.draw(|f| drawn = Some(draw(f, app, &ctx))).unwrap();
    (terminal.backend().buffer().clone(), drawn.unwrap())
}

fn screen(app: &App, width: u16, height: u16) -> Vec<String> {
    lines(&render_to(app, width, height).0)
}

fn all(app: &App, width: u16, height: u16) -> String {
    screen(app, width, height).join("\n")
}

#[test]
fn the_prompt_line_shows_the_query() {
    let mut app = three();
    type_text(&mut app, "ab");
    assert_eq!(screen(&app, 80, 20)[0], "> ab");
}

#[test]
fn the_status_line_shows_matches_and_sessions() {
    let mut app = three();
    type_text(&mut app, "one");
    assert!(screen(&app, 80, 20)[1].starts_with("2/3"));
}

#[test]
fn the_status_line_shows_indexing_until_done() {
    let mut app = App::new();
    let mut events = vec![IndexEvent::Total(4)];
    for (id, hour) in [(ID_A, 1), (ID_B, 2), (ID_C, 3)] {
        events.push(IndexEvent::Session(Box::new(session(id, &["w"], hour))));
    }
    let effects = app.on_index(events);
    settle(&mut app, effects);
    assert!(screen(&app, 80, 20)[1].contains("indexing 3/4"));
    app.on_index(vec![IndexEvent::Done]);
    assert!(!screen(&app, 80, 20)[1].contains("indexing"));
}

#[test]
fn the_status_line_shows_the_unreadable_count() {
    let mut app = three();
    app.on_index(vec![IndexEvent::Unreadable {
        path: PathBuf::from("/x/bad.jsonl"),
        reason: "no".to_string(),
    }]);
    assert!(screen(&app, 80, 20)[1].contains("1 unreadable"));
}

#[test]
fn a_row_shows_date_directory_and_title() {
    let app = three();
    let row = screen(&app, 80, 20)[2].clone();
    assert!(row.contains("2026-11-01 12:00"), "{row}");
    assert!(row.contains("~/code/x"), "{row}");
    assert!(row.contains("Title"), "{row}");
}

#[test]
fn a_directory_that_only_starts_like_home_stays_whole() {
    let mut s = session(ID_A, &["w"], 10);
    s.cwd = Some("/home/user2/x".to_string());
    let app = app_with(vec![s]);
    let row = screen(&app, 80, 20)[2].clone();
    assert!(row.contains("/home/user2/x"), "{row}");
    assert!(!row.contains('~'), "{row}");
}

#[test]
fn a_compacted_session_shows_the_count_on_its_row() {
    let app = app_with(vec![compacted(ID_A, &["w"], 0)]);
    assert!(screen(&app, 80, 20)[2].contains("⇊2"));
}

#[test]
fn a_session_that_is_not_compacted_shows_no_mark() {
    let app = three();
    let rows = screen(&app, 80, 20);
    assert!(rows[2..5].iter().all(|r| !r.contains('⇊')), "{rows:?}");
}

#[test]
fn the_selected_row_starts_with_a_marker() {
    let app = three();
    let rows = screen(&app, 80, 20);
    assert!(rows[2].starts_with("> "), "{rows:?}");
    assert!(rows[3].starts_with("  2026"), "{rows:?}");
    assert!(rows[4].starts_with("  2026"), "{rows:?}");
}

#[test]
fn a_wide_screen_puts_the_list_and_the_preview_side_by_side() {
    let app = three();
    let rows = screen(&app, 120, 30);
    assert!(
        rows.iter()
            .any(|r| r.contains("2026-11-01") && r.contains(ID_C)),
        "{rows:?}"
    );
}

#[test]
fn a_narrow_screen_puts_the_preview_below_the_list() {
    let app = three();
    let rows = screen(&app, 80, 30);
    let header = rows.iter().position(|r| r.contains(ID_C)).unwrap();
    let last_row = rows.iter().rposition(|r| r.contains("2026-11-01")).unwrap();
    assert!(header > last_row, "{rows:?}");
}

#[test]
fn the_preview_header_shows_id_directory_and_branch() {
    let app = three();
    let text = all(&app, 120, 30);
    assert!(text.contains(ID_C));
    assert!(text.contains("branch main"));
    let rows = screen(&app, 80, 30);
    let header = rows.iter().position(|r| r.contains(ID_C)).unwrap();
    assert!(rows[header + 1].contains("~/code/x"), "{rows:?}");
    assert!(rows[header + 2].contains("branch main"), "{rows:?}");
}

#[test]
fn a_missing_branch_shows_a_question_mark() {
    let mut s = session(ID_A, &["w"], 10);
    s.git_branch = None;
    let app = app_with(vec![s]);
    assert!(all(&app, 80, 30).contains("branch ?"));
}

#[test]
fn a_query_word_in_the_preview_has_the_highlight_style() {
    let app = app_with(vec![session(ID_A, &["needle in haystack"], 10)]);
    let mut app = app;
    type_text(&mut app, "needle");
    let (buffer, _) = render_to(&app, 80, 30);
    let mut found = false;
    for y in 4..buffer.area.height {
        for x in 0..buffer.area.width.saturating_sub(6) {
            let word: String = (0..6).map(|i| buffer[(x + i, y)].symbol()).collect();
            if word == "needle" {
                assert_eq!(buffer[(x, y)].bg, Color::Yellow);
                found = true;
            }
        }
    }
    assert!(found, "{:?}", lines(&buffer));
}

#[test]
fn the_question_shows_the_facts_and_the_choices() {
    let mut app = app_with(vec![compacted(ID_A, &["w"], 0)]);
    app.on_key(press(KeyCode::Enter));
    let text = all(&app, 100, 30);
    assert!(text.contains("2 compactions"), "{text}");
    assert!(text.contains("259,822 tokens"), "{text}");
    assert!(text.contains("> ch"), "{text}");
    assert!(text.contains("  che"), "{text}");
    app.on_key(press(KeyCode::Down));
    let text = all(&app, 100, 30);
    assert!(text.contains("> che"), "{text}");
}

#[test]
fn the_question_warns_when_the_match_comes_before_the_last_compaction() {
    let mut app = app_with(vec![compacted(ID_A, &["needle early", "later"], 1)]);
    type_text(&mut app, "needle");
    app.on_key(press(KeyCode::Enter));
    assert!(all(&app, 100, 30).contains("before the last compaction"));
}

#[test]
fn the_question_does_not_warn_when_the_match_comes_after_the_last_compaction() {
    let mut app = app_with(vec![compacted(ID_A, &["early", "needle late"], 1)]);
    type_text(&mut app, "needle");
    app.on_key(press(KeyCode::Enter));
    let text = all(&app, 100, 30);
    assert!(text.contains("2 compactions"), "{text}");
    assert!(!text.contains("before the last compaction"), "{text}");
}

#[test]
fn list_rows_is_the_height_of_the_list_area() {
    let app = three();
    // Wide: the body is the screen minus the prompt, status, and footer.
    assert_eq!(render_to(&app, 120, 30).1.list_rows, 27);
    assert_eq!(render_to(&app, 120, 10).1.list_rows, 7);
    // Narrow: the list takes the top half of the body.
    assert_eq!(render_to(&app, 80, 30).1.list_rows, 13);
    assert_eq!(render_to(&app, 80, 20).1.list_rows, 8);
}

#[test]
fn the_drawn_preview_scroll_is_clamped() {
    let mut app = three();
    for _ in 0..1000 {
        app.on_key(shift_up());
    }
    assert_eq!(app.preview_scroll(), -1000);
    let drawn = render_to(&app, 120, 30).1;
    assert!(drawn.preview_scroll > -1000, "{drawn:?}");
}

#[test]
fn no_size_makes_the_draw_panic() {
    let list = three();
    let mut ask = app_with(vec![compacted(ID_A, &["w"], 0)]);
    ask.on_key(press(KeyCode::Enter));
    for (w, h) in [(0, 0), (1, 1), (10, 3), (20, 5), (200, 60), (100, 2), (99, 4)] {
        for app in [&list, &ask, &App::new()] {
            let (_, drawn) = render_to(app, w, h);
            assert!(drawn.list_rows >= 1);
        }
    }
}

#[test]
fn wide_characters_in_a_row_do_not_panic_and_are_cut() {
    let mut s = session(ID_A, &["w"], 10);
    s.ai_title = Some("日本語のタイトル🎉".to_string());
    s.cwd = Some("/home/u/日本語/café".to_string());
    let app = app_with(vec![s]);
    let row = screen(&app, 40, 12)[2].clone();
    assert!(row.contains("~/") || row.contains('…'), "{row}");
}

#[test]
fn an_empty_list_says_what_it_waits_for() {
    let mut app = App::new();
    assert!(all(&app, 80, 20).contains("reading sessions…"));
    app.on_index(vec![IndexEvent::Done]);
    assert!(all(&app, 80, 20).contains("no session matches"));
}

#[test]
fn the_footer_names_the_keys() {
    let app = three();
    let rows = screen(&app, 120, 30);
    assert!(rows[29].contains("Enter resume"), "{rows:?}");
}
