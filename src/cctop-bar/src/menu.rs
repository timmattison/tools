//! The menu of the status item as data. The shell turns each entry into a
//! menu item, thus the order and the text live here and run on every platform.

use crate::chord::Chord;
use cctop::board::Board;
use cctop::focus::FocusTarget;
use cctop::state::EpochMillis;
use cctop::ui::{format_elapsed, title_counts};

/// The title of the item that shows cctop.
pub const SHOW_TITLE: &str = "Show cctop";
/// The title of the notifications switch.
pub const NOTIFICATIONS_TITLE: &str = "Notifications";
/// The title of the item that quits the program.
pub const QUIT_TITLE: &str = "Quit cctop-bar";

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// Disabled text: the counts of the cctop title line.
    Counts(String),
    /// A session that waits for the person. A click focuses it.
    Session {
        /// The label of the session and the time it waits.
        title: String,
        /// What a click focuses.
        target: FocusTarget,
    },
    /// "Show cctop", with the hotkey when there is one.
    Show(String),
    /// The notifications switch and its state.
    Notifications(bool),
    /// Disabled text: the last error.
    Error(String),
    /// The item that quits the program.
    Quit,
    /// A line between two groups.
    Separator,
}

/// What the menu shows.
pub struct MenuInput<'a> {
    /// The board of the sessions.
    pub board: &'a Board,
    /// The time now.
    pub now: EpochMillis,
    /// The state of the notifications switch.
    pub notify: bool,
    /// The global hotkey, when one is registered.
    pub hotkey: Option<Chord>,
    /// The text of the last error, when there is one.
    pub last_error: Option<&'a str>,
}

/// The entries of the menu, top to bottom.
#[must_use]
pub fn entries(input: &MenuInput<'_>) -> Vec<MenuEntry> {
    let mut list = vec![
        MenuEntry::Counts(title_counts(input.board)),
        MenuEntry::Separator,
    ];
    let waiting: Vec<MenuEntry> = input
        .board
        .rows()
        .into_iter()
        .filter(|session| session.waits_for_you())
        .map(|session| MenuEntry::Session {
            title: format!(
                "{} · {}",
                session.label(),
                format_elapsed(input.now.saturating_since(session.since))
            ),
            target: FocusTarget::of(session),
        })
        .collect();
    if !waiting.is_empty() {
        list.extend(waiting);
        list.push(MenuEntry::Separator);
    }
    list.push(MenuEntry::Show(match input.hotkey {
        None => SHOW_TITLE.to_string(),
        Some(chord) => format!("{SHOW_TITLE}  {}", chord.display()),
    }));
    list.push(MenuEntry::Notifications(input.notify));
    if let Some(text) = input.last_error {
        list.push(MenuEntry::Error(format!("Error: {text}")));
    }
    list.push(MenuEntry::Separator);
    list.push(MenuEntry::Quit);
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctop::liveness::RunStatus;
    use cctop::state::parse_state_file;
    use cctop::ui::{format_elapsed, title_counts};

    const NOW: u64 = 1_000_000;

    fn file_json(pid: u32, state: &str, directory: &str, stamp: u64) -> String {
        format!(
            r#"{{"version":1,"pid":{pid},"sessionId":"s{pid}","startedAt":100,"state":"{state}","stamp":{stamp},"cwd":"/path/to/{directory}"}}"#
        )
    }

    fn load(board: &mut Board, pid: u32, state: &str, directory: &str, stamp: u64) {
        let file = parse_state_file(&file_json(pid, state, directory, stamp)).unwrap();
        board.load(file, None);
    }

    fn input(board: &Board) -> MenuInput<'_> {
        MenuInput {
            board,
            now: EpochMillis::new(NOW),
            notify: true,
            hotkey: None,
            last_error: None,
        }
    }

    fn sessions(entries: &[MenuEntry]) -> Vec<(&str, &FocusTarget)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Session { title, target } => Some((title.as_str(), target)),
                _ => None,
            })
            .collect()
    }

    fn mixed_board() -> Board {
        let mut board = Board::new();
        load(&mut board, 1, "idle", "café", NOW - 5_000);
        load(&mut board, 2, "needs-you", "日本語", NOW - 125_000);
        load(&mut board, 3, "working", "🎉", NOW - 1_000);
        load(&mut board, 4, "needs-you", "handled", NOW - 9_000);
        load(&mut board, 5, "needs-you", "frozen", NOW - 9_000);
        load(&mut board, 6, "needs-you", "café", NOW - 3_000);
        let handled = parse_state_file(&file_json(4, "needs-you", "handled", 0))
            .unwrap()
            .pid;
        board.toggle_handled(handled);
        let frozen = parse_state_file(&file_json(5, "needs-you", "frozen", 0))
            .unwrap()
            .pid;
        board.set_run_status(frozen, RunStatus::Stopped, EpochMillis::new(NOW));
        board
    }

    // Behavior 12.
    #[test]
    fn counts_come_first_then_a_separator() {
        let board = mixed_board();
        let list = entries(&input(&board));
        assert_eq!(list[0], MenuEntry::Counts(title_counts(&board)));
        assert_eq!(list[1], MenuEntry::Separator);
    }

    #[test]
    fn one_session_entry_for_each_waiting_session_in_row_order() {
        let board = mixed_board();
        let list = entries(&input(&board));
        let found = sessions(&list);
        let waiting: Vec<_> = board
            .rows()
            .into_iter()
            .filter(|s| s.waits_for_you())
            .collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found.len(), board.attention_count());
        for ((title, target), session) in found.iter().zip(waiting) {
            let elapsed = format_elapsed(EpochMillis::new(NOW).saturating_since(session.since));
            assert_eq!(*title, format!("{} · {elapsed}", session.label()));
            assert_eq!(**target, FocusTarget::of(session));
        }
        assert!(found[0].0.starts_with("日本語 · ") || found[0].0.starts_with("café · "));
        let labels: Vec<_> = found.iter().map(|(title, _)| *title).collect();
        assert!(labels
            .iter()
            .any(|title| title.starts_with("日本語 · 2m 05s")));
        assert!(labels.iter().any(|title| title.starts_with("café · 3s")));
    }

    #[test]
    fn handled_frozen_and_other_states_give_no_session() {
        let board = mixed_board();
        let list = entries(&input(&board));
        let titles: Vec<_> = sessions(&list)
            .iter()
            .map(|(title, _)| (*title).to_string())
            .collect();
        assert!(titles.iter().all(|title| !title.starts_with("handled")));
        assert!(titles.iter().all(|title| !title.starts_with("frozen")));
        assert!(titles.iter().all(|title| !title.starts_with('🎉')));
        assert_eq!(titles.len(), 2);
    }

    #[test]
    fn a_separator_follows_the_sessions_only_when_some_came() {
        let board = mixed_board();
        let list = entries(&input(&board));
        let last_session = list
            .iter()
            .rposition(|entry| matches!(entry, MenuEntry::Session { .. }))
            .unwrap();
        assert_eq!(list[last_session + 1], MenuEntry::Separator);
        assert!(matches!(list[last_session + 2], MenuEntry::Show(_)));
    }

    // Behavior 13.
    #[test]
    fn show_has_the_hotkey_when_there_is_one() {
        let board = Board::new();
        let plain = entries(&input(&board));
        assert!(plain.contains(&MenuEntry::Show(SHOW_TITLE.to_string())));
        let mut with = input(&board);
        with.hotkey = Some(Chord::default());
        assert!(entries(&with).contains(&MenuEntry::Show(format!("{SHOW_TITLE}  ⌃⌥⇧⌘⌫"))));
    }

    #[test]
    fn notifications_carry_the_switch() {
        let board = Board::new();
        let mut on = input(&board);
        assert!(entries(&on).contains(&MenuEntry::Notifications(true)));
        on.notify = false;
        assert!(entries(&on).contains(&MenuEntry::Notifications(false)));
    }

    #[test]
    fn an_error_appears_only_with_a_last_error() {
        let board = Board::new();
        let mut shown = input(&board);
        assert!(!entries(&shown)
            .iter()
            .any(|e| matches!(e, MenuEntry::Error(_))));
        shown.last_error = Some("日本語 failed");
        let list = entries(&shown);
        assert!(list.contains(&MenuEntry::Error("Error: 日本語 failed".to_string())));
        let error = list
            .iter()
            .position(|e| matches!(e, MenuEntry::Error(_)))
            .unwrap();
        assert_eq!(list[error - 1], MenuEntry::Notifications(true));
        assert_eq!(list[error + 1], MenuEntry::Separator);
    }

    #[test]
    fn the_list_ends_with_a_separator_and_quit() {
        let board = mixed_board();
        let list = entries(&input(&board));
        assert_eq!(
            list[list.len() - 2..],
            [MenuEntry::Separator, MenuEntry::Quit]
        );
    }

    #[test]
    fn with_no_waiting_session_no_separator_is_doubled() {
        let mut board = Board::new();
        load(&mut board, 1, "working", "café", NOW);
        let list = entries(&input(&board));
        assert_eq!(
            list,
            vec![
                MenuEntry::Counts(title_counts(&board)),
                MenuEntry::Separator,
                MenuEntry::Show(SHOW_TITLE.to_string()),
                MenuEntry::Notifications(true),
                MenuEntry::Separator,
                MenuEntry::Quit,
            ]
        );
        assert!(list
            .windows(2)
            .all(|pair| pair != [MenuEntry::Separator, MenuEntry::Separator]));
    }

    #[test]
    fn the_full_menu_has_this_order() {
        let mut board = Board::new();
        load(&mut board, 2, "needs-you", "🎉", NOW - 61_000);
        let mut full = input(&board);
        full.hotkey = Some(Chord::default());
        full.last_error = Some("boom");
        let list = entries(&full);
        let target = FocusTarget::of(board.rows()[0]);
        assert_eq!(
            list,
            vec![
                MenuEntry::Counts(title_counts(&board)),
                MenuEntry::Separator,
                MenuEntry::Session {
                    title: "🎉 · 1m 01s".to_string(),
                    target
                },
                MenuEntry::Separator,
                MenuEntry::Show(format!("{SHOW_TITLE}  ⌃⌥⇧⌘⌫")),
                MenuEntry::Notifications(true),
                MenuEntry::Error("Error: boom".to_string()),
                MenuEntry::Separator,
                MenuEntry::Quit,
            ]
        );
    }
}
