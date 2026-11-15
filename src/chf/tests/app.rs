//! Behavior tests for `chf::app`.
#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::unwrap_used,
    reason = "every unwrap in this file is an assertion, not an unhandled error"
)]

use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use chf::app::{App, Drawn, Effect, Mode, Progress, Version};
use chf::index::IndexEvent;
use chf::search::run_job;
use chf::session::{MessageSpan, Role, Session};
use chrono::{TimeZone, Utc};
use claude_transcript::{CompactionFacts, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

const ID_A: &str = "00000000-0000-4000-8000-00000000000a";
const ID_B: &str = "00000000-0000-4000-8000-00000000000b";
const ID_C: &str = "00000000-0000-4000-8000-00000000000c";
const ID_D: &str = "00000000-0000-4000-8000-00000000000d";

fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }
}

fn press(code: KeyCode) -> KeyEvent {
    key(code, KeyModifiers::NONE, KeyEventKind::Press)
}

fn ctrl(c: char) -> KeyEvent {
    key(KeyCode::Char(c), KeyModifiers::CONTROL, KeyEventKind::Press)
}

fn shift(code: KeyCode) -> KeyEvent {
    key(code, KeyModifiers::SHIFT, KeyEventKind::Press)
}

fn release(code: KeyCode) -> KeyEvent {
    key(code, KeyModifiers::NONE, KeyEventKind::Release)
}

fn ch(c: char) -> KeyEvent {
    press(KeyCode::Char(c))
}

/// A session with `word` as its only message. `hour` sets `last_activity`.
fn session(id: &str, word: &str, hour: u32, compacted: bool) -> Session {
    Session {
        id: SessionId::parse(id).unwrap(),
        path: PathBuf::from("/x/session.jsonl"),
        cwd: None,
        git_branch: None,
        ai_title: None,
        last_activity: Utc.with_ymd_and_hms(2026, 11, 9, hour, 0, 0).unwrap(),
        compaction: CompactionFacts {
            compacted,
            compactions: usize::from(compacted),
            last_pre_tokens: None,
        },
        text: word.to_string(),
        messages: vec![MessageSpan {
            role: Role::User,
            start: 0,
            end: word.len(),
        }],
        compaction_points: Vec::new(),
    }
}

fn event(id: &str, word: &str, hour: u32, compacted: bool) -> IndexEvent {
    IndexEvent::Session(Box::new(session(id, word, hour, compacted)))
}

/// Run each search effect now and give the hits to the app. Return the rest.
fn settle(app: &mut App, effects: Vec<Effect>) -> Vec<Effect> {
    let mut rest = Vec::new();
    for effect in effects {
        match effect {
            Effect::Search(job) => {
                let hits = run_job(&job, &AtomicU64::new(job.generation)).unwrap();
                app.on_hits(hits);
            }
            other => rest.push(other),
        }
    }
    rest
}

fn searches(effects: &[Effect]) -> Vec<&chf::search::Job> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Search(job) => Some(job),
            _ => None,
        })
        .collect()
}

fn id_of(app: &App, row: usize) -> String {
    app.session(app.hits()[row].session).id.as_str().to_string()
}

fn ids(app: &App) -> Vec<String> {
    (0..app.hits().len()).map(|row| id_of(app, row)).collect()
}

/// An app with sessions a (hour 1), b (hour 2), c (hour 3), all settled.
fn loaded() -> App {
    let mut app = App::new();
    let effects = app.on_index(vec![
        event(ID_A, "alpha", 1, false),
        event(ID_B, "beta", 2, false),
        event(ID_C, "gamma", 3, false),
    ]);
    settle(&mut app, effects);
    app
}

/// An app with one compacted session, settled.
fn loaded_compacted() -> App {
    let mut app = App::new();
    let effects = app.on_index(vec![event(ID_A, "alpha", 1, true)]);
    settle(&mut app, effects);
    app
}

#[test]
fn index_events_fill_progress_and_give_one_search() {
    let mut app = App::new();
    let effects = app.on_index(vec![
        IndexEvent::Total(3),
        event(ID_A, "alpha", 1, false),
        event(ID_B, "beta", 2, false),
        IndexEvent::Unreadable {
            path: PathBuf::from("/x/bad.jsonl"),
            reason: "denied".to_string(),
        },
        event(ID_C, "gamma", 3, false),
        IndexEvent::Done,
    ]);
    assert_eq!(
        app.progress(),
        Progress {
            read: 4,
            total: Some(3),
            done: true,
            unreadable: 1
        }
    );
    let jobs = searches(&effects);
    assert_eq!(effects.len(), 1);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].generation, 0);
    let indexes: Vec<usize> = jobs[0].sessions.iter().map(|(i, _)| *i).collect();
    assert_eq!(indexes, vec![0, 1, 2]);
    assert_eq!(app.sessions_len(), 3);
    settle(&mut app, effects);
    assert_eq!(ids(&app), vec![ID_C, ID_B, ID_A]);
}

#[test]
fn a_batch_without_sessions_gives_no_effect() {
    let mut app = App::new();
    let effects = app.on_index(vec![IndexEvent::Total(0), IndexEvent::Done]);
    assert!(effects.is_empty());
    assert!(app.progress().done);
}

#[test]
fn a_second_batch_searches_only_the_new_sessions() {
    let mut app = loaded();
    let effects = app.on_index(vec![event(ID_D, "delta", 2, false)]);
    let jobs = searches(&effects);
    assert_eq!(jobs.len(), 1);
    let indexes: Vec<usize> = jobs[0].sessions.iter().map(|(i, _)| *i).collect();
    assert_eq!(indexes, vec![3]);
    settle(&mut app, effects);
    // hour 2 ties with b: the smaller id comes first.
    assert_eq!(ids(&app), vec![ID_C, ID_B, ID_D, ID_A]);
}

#[test]
fn typing_gives_a_search_for_each_generation() {
    let mut app = loaded();
    let first = app.on_key(ch('a'));
    let second = app.on_key(ch('b'));
    assert_eq!(app.query_text(), "ab");
    let (j1, j2) = (searches(&first), searches(&second));
    assert_eq!((j1.len(), j2.len()), (1, 1));
    assert_eq!((j1[0].generation, j2[0].generation), (1, 2));
    assert_eq!(j2[0].sessions.len(), 3);
    assert_eq!(app.generation(), 2);
    assert!(!app.query().is_empty());
}

#[test]
fn shifted_characters_type() {
    let mut app = loaded();
    let effects = app.on_key(key(
        KeyCode::Char('A'),
        KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));
    assert_eq!(app.query_text(), "A");
    assert_eq!(searches(&effects).len(), 1);
}

#[test]
fn backspace_removes_a_character_and_empty_backspace_does_nothing() {
    let mut app = loaded();
    app.on_key(ch('a'));
    app.on_key(ch('b'));
    let effects = app.on_key(press(KeyCode::Backspace));
    assert_eq!(app.query_text(), "a");
    assert_eq!(searches(&effects)[0].generation, 3);
    app.on_key(press(KeyCode::Backspace));
    assert_eq!(app.generation(), 4);
    let effects = app.on_key(press(KeyCode::Backspace));
    assert!(effects.is_empty());
    assert_eq!(app.generation(), 4);
}

#[test]
fn backspace_removes_a_whole_multibyte_character() {
    let mut app = loaded();
    app.on_key(ch('日'));
    app.on_key(ch('é'));
    app.on_key(press(KeyCode::Backspace));
    assert_eq!(app.query_text(), "日");
}

#[test]
fn ctrl_u_clears_the_query() {
    let mut app = loaded();
    app.on_key(ch('a'));
    app.on_key(ch('b'));
    let effects = app.on_key(ctrl('u'));
    assert_eq!(app.query_text(), "");
    assert_eq!(searches(&effects)[0].generation, 3);
    let effects = app.on_key(ctrl('u'));
    assert!(effects.is_empty());
}

#[test]
fn hits_of_an_old_generation_are_dropped() {
    let mut app = loaded();
    let e1 = app.on_key(ch('a'));
    let e2 = app.on_key(ch('l'));
    let old = run_job(searches(&e1)[0], &AtomicU64::new(1)).unwrap();
    assert_eq!(old.generation, 1);
    app.on_hits(old);
    assert_eq!(ids(&app), vec![ID_C, ID_B, ID_A], "the list is unchanged");
    settle(&mut app, e2);
    assert_eq!(ids(&app), vec![ID_A]);
}

#[test]
fn the_first_hits_of_a_new_generation_replace_the_list() {
    let mut app = loaded();
    let effects = app.on_key(ch('g'));
    settle(&mut app, effects);
    assert_eq!(ids(&app), vec![ID_C]);
}

#[test]
fn a_new_generation_with_no_hit_empties_the_list() {
    let mut app = loaded();
    let effects = app.on_key(ch('z'));
    settle(&mut app, effects);
    assert!(app.hits().is_empty());
    assert_eq!(app.selected(), 0);
    assert!(app.selected_hit().is_none());
}

#[test]
fn later_hits_of_the_same_generation_merge() {
    let mut app = loaded();
    let effects = app.on_key(ch('a'));
    settle(&mut app, effects);
    // "alpha", "beta", "gamma" all hold "a".
    assert_eq!(app.hits().len(), 3);
    let more = app.on_index(vec![event(ID_D, "delta", 5, false)]);
    settle(&mut app, more);
    assert_eq!(ids(&app), vec![ID_D, ID_C, ID_B, ID_A]);
}

#[test]
fn the_selection_follows_its_session_when_a_newer_one_arrives() {
    let mut app = loaded();
    app.on_key(press(KeyCode::Down));
    assert_eq!(id_of(&app, app.selected()), ID_B);
    app.on_key(shift(KeyCode::Down));
    let effects = app.on_index(vec![event(ID_D, "delta", 9, false)]);
    settle(&mut app, effects);
    assert_eq!(app.selected(), 2);
    assert_eq!(id_of(&app, app.selected()), ID_B);
    assert_eq!(app.preview_scroll(), 1, "the scroll stays");
}

/// An app with sessions a (hour 1) and b (hour 2), settled.
fn loaded_first_batch() -> App {
    let mut app = App::new();
    let effects = app.on_index(vec![
        event(ID_A, "alpha", 1, false),
        event(ID_B, "beta", 2, false),
    ]);
    settle(&mut app, effects);
    app
}

#[test]
fn an_untouched_selection_stays_on_the_newest_session() {
    let mut app = loaded_first_batch();
    let effects = app.on_index(vec![event(ID_D, "delta", 9, false)]);
    settle(&mut app, effects);
    assert_eq!(app.selected(), 0);
    assert_eq!(id_of(&app, 0), ID_D);
}

#[test]
fn enter_resumes_the_newest_match_when_the_person_did_not_move() {
    let mut app = loaded_first_batch();
    let effects = app.on_index(vec![event(ID_D, "delta", 9, false)]);
    settle(&mut app, effects);
    // "alpha", "beta" and "delta" all hold an "a".
    let effects = app.on_key(ch('a'));
    settle(&mut app, effects);
    let effects = app.on_key(press(KeyCode::Enter));
    assert_eq!(launch_of(&effects).0, ID_D);
}

#[test]
fn an_untouched_selection_resets_the_scroll_when_a_newer_session_takes_row_0() {
    let mut app = loaded_first_batch();
    app.on_key(shift(KeyCode::Down));
    assert_eq!(app.preview_scroll(), 1);
    let effects = app.on_index(vec![event(ID_D, "delta", 9, false)]);
    settle(&mut app, effects);
    assert_eq!(app.preview_scroll(), 0);
}

#[test]
fn an_untouched_selection_keeps_the_scroll_when_row_0_does_not_change() {
    let mut app = loaded_first_batch();
    app.on_key(shift(KeyCode::Down));
    let effects = app.on_index(vec![event(ID_C, "gamma", 0, false)]);
    settle(&mut app, effects);
    assert_eq!(app.selected(), 0);
    assert_eq!(id_of(&app, 0), ID_B);
    assert_eq!(app.preview_scroll(), 1);
}

#[test]
fn the_selection_keeps_its_row_when_its_session_leaves() {
    let mut app = loaded();
    app.on_key(press(KeyCode::Down));
    app.on_key(shift(KeyCode::Down));
    assert_eq!(id_of(&app, app.selected()), ID_B);
    // Only "gamma" matches: b leaves. Row 1 is past the end, so it clamps to 0.
    let effects = app.on_key(ch('g'));
    settle(&mut app, effects);
    assert_eq!(app.selected(), 0);
    assert_eq!(app.preview_scroll(), 0);
}

#[test]
fn the_selection_clamps_when_the_list_shrinks_below_its_row() {
    let mut app = loaded();
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::Down));
    assert_eq!(id_of(&app, 2), ID_A);
    // Only "beta" holds a "t", so the list has one row and row 2 clamps to 0.
    let effects = app.on_key(ch('t'));
    settle(&mut app, effects);
    assert_eq!(ids(&app), vec![ID_B]);
    assert_eq!(app.selected(), 0);
}

#[test]
fn arrows_and_control_keys_move_and_stop_at_the_ends() {
    let mut app = loaded();
    app.on_key(press(KeyCode::Up));
    assert_eq!(app.selected(), 0);
    app.on_key(press(KeyCode::Down));
    assert_eq!(app.selected(), 1);
    app.on_key(ctrl('n'));
    assert_eq!(app.selected(), 2);
    app.on_key(ctrl('n'));
    app.on_key(press(KeyCode::Down));
    assert_eq!(app.selected(), 2);
    app.on_key(ctrl('p'));
    assert_eq!(app.selected(), 1);
    app.on_key(press(KeyCode::Up));
    app.on_key(press(KeyCode::Up));
    assert_eq!(app.selected(), 0);
    assert_eq!(app.query_text(), "", "control keys do not type");
}

#[test]
fn a_move_on_an_empty_list_does_nothing() {
    let mut app = App::new();
    app.on_key(press(KeyCode::Down));
    app.on_key(press(KeyCode::PageDown));
    assert_eq!(app.selected(), 0);
}

#[test]
fn page_keys_move_by_the_rows_of_the_list() {
    let mut app = App::new();
    let events: Vec<IndexEvent> = (0..6)
        .map(|n| {
            let id = format!("00000000-0000-4000-8000-0000000001{n:02}");
            event(&id, "word", n + 1, false)
        })
        .collect();
    let effects = app.on_index(events);
    settle(&mut app, effects);
    assert_eq!(app.list_rows(), 1);
    app.on_drawn(Drawn {
        list_rows: 2,
        preview_scroll: 0,
    });
    assert_eq!(app.list_rows(), 2);
    app.on_key(press(KeyCode::PageDown));
    assert_eq!(app.selected(), 2);
    assert_eq!(app.list_offset(), 1);
    app.on_key(press(KeyCode::PageDown));
    app.on_key(press(KeyCode::PageDown));
    app.on_key(press(KeyCode::PageDown));
    assert_eq!(app.selected(), 5);
    assert_eq!(app.list_offset(), 4);
    app.on_key(press(KeyCode::PageUp));
    assert_eq!(app.selected(), 3);
    assert_eq!(app.list_offset(), 3);
    app.on_key(press(KeyCode::PageUp));
    app.on_key(press(KeyCode::PageUp));
    assert_eq!(app.selected(), 0);
    assert_eq!(app.list_offset(), 0);
}

#[test]
fn on_drawn_makes_at_least_one_row() {
    let mut app = loaded();
    app.on_drawn(Drawn {
        list_rows: 0,
        preview_scroll: 0,
    });
    assert_eq!(app.list_rows(), 1);
}

#[test]
fn shift_arrows_scroll_the_preview_and_a_move_resets_it() {
    let mut app = loaded();
    app.on_key(shift(KeyCode::Down));
    app.on_key(shift(KeyCode::Down));
    assert_eq!(app.preview_scroll(), 2);
    app.on_key(shift(KeyCode::Up));
    assert_eq!(app.preview_scroll(), 1);
    assert_eq!(app.selected(), 0, "shift does not move the selection");
    app.on_key(press(KeyCode::Down));
    assert_eq!(app.preview_scroll(), 0);
}

#[test]
fn a_query_change_resets_the_preview_scroll() {
    let mut app = loaded();
    app.on_key(shift(KeyCode::Down));
    app.on_key(ch('a'));
    assert_eq!(app.preview_scroll(), 0);
}

#[test]
fn on_drawn_stores_the_clamped_scroll() {
    let mut app = loaded();
    app.on_key(shift(KeyCode::Down));
    app.on_key(shift(KeyCode::Down));
    app.on_drawn(Drawn {
        list_rows: 5,
        preview_scroll: 1,
    });
    assert_eq!(app.preview_scroll(), 1);
}

#[test]
fn enter_launches_a_session_without_compaction() {
    let mut app = loaded();
    let effects = app.on_key(press(KeyCode::Enter));
    match effects.as_slice() {
        [Effect::Launch { id, version }] => {
            assert_eq!(id.as_str(), ID_C);
            assert_eq!(*version, Version::Compacted);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn enter_on_a_compacted_session_asks() {
    let mut app = loaded_compacted();
    let effects = app.on_key(press(KeyCode::Enter));
    assert!(effects.is_empty());
    assert_eq!(
        app.mode(),
        Mode::Ask {
            session: 0,
            choice: Version::Compacted
        }
    );
}

fn launch_of(effects: &[Effect]) -> (String, Version) {
    match effects {
        [Effect::Launch { id, version }] => (id.as_str().to_string(), *version),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_question_takes_c() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    let effects = app.on_key(ch('c'));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Compacted));
}

#[test]
fn the_question_takes_e() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    let effects = app.on_key(ch('e'));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Expanded));
}

#[test]
fn the_question_switches_with_down_and_up_then_enter_chooses() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    app.on_key(press(KeyCode::Down));
    assert_eq!(
        app.mode(),
        Mode::Ask {
            session: 0,
            choice: Version::Expanded
        }
    );
    let effects = app.on_key(press(KeyCode::Enter));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Expanded));

    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    app.on_key(press(KeyCode::Up));
    app.on_key(press(KeyCode::Up));
    let effects = app.on_key(press(KeyCode::Enter));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Compacted));
}

#[test]
fn the_question_enter_takes_the_default() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    let effects = app.on_key(press(KeyCode::Enter));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Compacted));
}

#[test]
fn escape_closes_the_question_and_keeps_the_state() {
    let mut app = loaded_compacted();
    app.on_key(ch('a'));
    let before = app.selected_hit().cloned();
    let text = app.query_text().to_string();
    app.on_key(press(KeyCode::Enter));
    let effects = app.on_key(press(KeyCode::Esc));
    assert!(effects.is_empty());
    assert_eq!(app.mode(), Mode::List);
    assert_eq!(app.query_text(), text);
    assert_eq!(app.selected_hit().cloned(), before);
}

#[test]
fn typing_in_the_question_changes_nothing() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    let mode = app.mode();
    let effects = app.on_key(ch('x'));
    assert!(effects.is_empty());
    assert_eq!(app.query_text(), "");
    assert_eq!(app.mode(), mode);
    assert_eq!(app.generation(), 0);
}

#[test]
fn the_question_launches_the_session_it_asked_about() {
    let mut app = App::new();
    let effects = app.on_index(vec![
        event(ID_A, "alpha", 1, true),
        event(ID_B, "beta", 2, false),
    ]);
    settle(&mut app, effects);
    app.on_key(press(KeyCode::Down));
    assert_eq!(id_of(&app, app.selected()), ID_A);
    app.on_key(press(KeyCode::Enter));
    // A newer session arrives, and the selected row changes.
    let effects = app.on_index(vec![event(ID_C, "gamma", 9, false)]);
    settle(&mut app, effects);
    let effects = app.on_key(ch('e'));
    assert_eq!(launch_of(&effects), (ID_A.to_string(), Version::Expanded));
}

#[test]
fn escape_and_ctrl_c_quit_from_the_list() {
    let mut app = loaded();
    assert!(matches!(
        app.on_key(press(KeyCode::Esc)).as_slice(),
        [Effect::Quit]
    ));
    assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Effect::Quit]));
}

#[test]
fn ctrl_c_quits_from_the_question() {
    let mut app = loaded_compacted();
    app.on_key(press(KeyCode::Enter));
    assert!(matches!(app.on_key(ctrl('c')).as_slice(), [Effect::Quit]));
}

#[test]
fn enter_on_an_empty_list_does_nothing() {
    let mut app = App::new();
    assert!(app.on_key(press(KeyCode::Enter)).is_empty());
    assert_eq!(app.mode(), Mode::List);
}

#[test]
fn other_keys_do_nothing() {
    let mut app = loaded();
    assert!(app.on_key(press(KeyCode::F(5))).is_empty());
    assert!(app.on_key(press(KeyCode::Tab)).is_empty());
    assert!(app
        .on_key(key(
            KeyCode::Char('x'),
            KeyModifiers::ALT,
            KeyEventKind::Press
        ))
        .is_empty());
    assert_eq!(app.query_text(), "");
}

#[test]
fn a_release_event_does_nothing() {
    let mut app = loaded();
    for code in [
        KeyCode::Char('a'),
        KeyCode::Down,
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Backspace,
    ] {
        assert!(app.on_key(release(code)).is_empty());
    }
    assert_eq!(app.query_text(), "");
    assert_eq!(app.selected(), 0);
    assert_eq!(app.generation(), 0);
    assert_eq!(app.mode(), Mode::List);
}

#[test]
fn a_repeat_event_acts() {
    let mut app = loaded();
    app.on_key(key(KeyCode::Down, KeyModifiers::NONE, KeyEventKind::Repeat));
    assert_eq!(app.selected(), 1);
}

#[test]
fn the_catalog_session_is_readable_by_index() {
    let app = loaded();
    assert_eq!(app.session(1).id.as_str(), ID_B);
    let _: &Arc<Session> = app.session(0);
}
