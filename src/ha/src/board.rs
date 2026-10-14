//! The board: the runs of all watched repositories, as rows for one screen.
//!
//! The board holds the last good runs of each repository, the last error of
//! each repository, the time window, the selection, and the scroll offset.
//! It does no I/O. Every reader takes the current time as an argument, so a
//! test uses one fixed time and never reads the clock.

#![allow(
    unused_variables,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    reason = "stubs for the red steps of the test-first build"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::run::{Run, RunId, RunState};
use crate::target::RepoName;

/// One of the two lists on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Runs that wait or run.
    Active,
    /// Runs that finished inside the window.
    Recent,
}

/// One row of the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row<'a> {
    /// The title row of a section.
    Header {
        /// The section that the header opens.
        section: Section,
        /// The number of runs in the section.
        count: usize,
        /// The number of passed runs. This is 0 for the active section.
        passed: usize,
        /// The number of failed runs. This is 0 for the active section.
        failed: usize,
    },
    /// The row that shows an empty section.
    Empty(Section),
    /// One run.
    Run {
        /// The run.
        run: &'a Run,
        /// True for the one row that the selection marks.
        selected: bool,
    },
}

/// The rows that fit on the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View<'a> {
    /// The visible slice of the rows.
    pub rows: Vec<Row<'a>>,
    /// The index of the first visible row in the full list of rows.
    pub first: usize,
}

/// The state of the board.
#[derive(Debug)]
pub struct Board {
    window: Duration,
    runs: BTreeMap<RepoName, Vec<Run>>,
    errors: BTreeMap<RepoName, String>,
    selected: Option<(RunId, usize)>,
}

impl Board {
    /// Makes an empty board. A finished run shows while it ended inside `window`.
    #[must_use]
    pub fn new(window: Duration) -> Board {
        Board {
            window,
            runs: BTreeMap::new(),
            errors: BTreeMap::new(),
            selected: None,
        }
    }

    /// The time window of the finished runs.
    #[must_use]
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Stores the runs of one repository and clears the error of that repository.
    pub fn replace(&mut self, repo: RepoName, runs: Vec<Run>) {
        self.errors.remove(&repo);
        self.runs.insert(repo, runs);
    }

    /// Records an error for one repository. The last good runs stay.
    pub fn fail(&mut self, repo: RepoName, error: String) {
        self.errors.insert(repo, error);
    }

    /// Drops the runs and the errors of every repository that is not in `repos`.
    pub fn keep_only(&mut self, repos: &BTreeSet<RepoName>) {
        self.runs.retain(|repo, _| repos.contains(repo));
        self.errors.retain(|repo, _| repos.contains(repo));
    }

    /// The error of each failed repository, sorted by repository.
    #[must_use]
    pub fn errors(&self) -> Vec<(&RepoName, &str)> {
        self.errors
            .iter()
            .map(|(repo, error)| (repo, error.as_str()))
            .collect()
    }

    /// All rows of the board at time `now`.
    pub fn rows(&mut self, now: DateTime<Utc>) -> Vec<Row<'_>> {
        self.settle(now);
        self.build_rows(now)
    }

    /// The rows that fit in `height` lines, with the selected row inside.
    pub fn view(&mut self, now: DateTime<Utc>, height: usize) -> View<'_> {
        View {
            rows: Vec::new(),
            first: 0,
        }
    }

    /// Splits the shown runs into the active list and the recent list.
    ///
    /// Both lists are sorted. A run that is not shown at `now` is in neither.
    fn sections(&self, now: DateTime<Utc>) -> (Vec<&Run>, Vec<&Run>) {
        let mut active: Vec<&Run> = self
            .runs
            .values()
            .flatten()
            .filter(|run| run.state.is_active())
            .collect();
        active.sort_by_key(|run| (run.started_at, run.id));
        let oldest = chrono::Duration::from_std(self.window)
            .ok()
            .and_then(|window| now.checked_sub_signed(window))
            .unwrap_or(DateTime::<Utc>::MIN_UTC);
        let mut recent: Vec<&Run> = self
            .runs
            .values()
            .flatten()
            .filter(|run| run.state.is_finished() && run.updated_at >= oldest)
            .collect();
        recent.sort_by_key(|run| std::cmp::Reverse((run.updated_at, run.id)));
        (active, recent)
    }

    /// All runs that show at `now`, in the order of the board: active, then recent.
    fn selectable(&self, now: DateTime<Utc>) -> Vec<&Run> {
        let (active, recent) = self.sections(now);
        active.into_iter().chain(recent).collect()
    }

    /// Makes the selection valid for the runs that show at `now`.
    ///
    /// The selection follows its run. When the run is gone, the run at the old
    /// position is selected, or the last run when the old position is past the
    /// end. A board with no run has no selection. A new selection is the first run.
    fn settle(&mut self, now: DateTime<Utc>) {
        let runs = self.selectable(now);
        let position = match self.selected {
            None => 0,
            Some((id, old)) => runs
                .iter()
                .position(|run| run.id == id)
                .unwrap_or_else(|| old.min(runs.len().saturating_sub(1))),
        };
        self.selected = runs.get(position).map(|run| (run.id, position));
    }

    /// Builds the full list of rows. The list has two sections.
    ///
    /// `selected` is the position of the selected run among all runs.
    fn build_rows(&self, now: DateTime<Utc>) -> Vec<Row<'_>> {
        let (active, recent) = self.sections(now);
        let passed = recent
            .iter()
            .filter(|run| run.state == RunState::Passed)
            .count();
        let mut rows = vec![Row::Header {
            section: Section::Active,
            count: active.len(),
            passed: 0,
            failed: 0,
        }];
        let selected = self.selected.map(|(_, index)| index);
        push_runs(&mut rows, &active, Section::Active, 0, selected);
        rows.push(Row::Header {
            section: Section::Recent,
            count: recent.len(),
            passed,
            failed: recent.len() - passed,
        });
        push_runs(&mut rows, &recent, Section::Recent, active.len(), selected);
        rows
    }

    /// Moves the selection to the next run.
    pub fn select_next(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        self.move_selection(now, |index, count| (index + 1).min(count - 1));
    }

    /// Moves the selection to the previous run.
    pub fn select_previous(&mut self, now: DateTime<Utc>) {
        self.settle(now);
        self.move_selection(now, |index, _| index.saturating_sub(1));
    }

    /// Moves the selection to the run at the position that `target` gives.
    ///
    /// `target` gets the current position and the number of runs.
    fn move_selection(&mut self, now: DateTime<Utc>, target: impl Fn(usize, usize) -> usize) {
        let runs = self.selectable(now);
        let moved = self.selected.and_then(|(_, index)| {
            let position = target(index, runs.len());
            runs.get(position).map(|run| (run.id, position))
        });
        self.selected = moved;
    }

    /// The address of the selected run, or `None` when the board has no run.
    pub fn selected_url(&mut self, now: DateTime<Utc>) -> Option<String> {
        self.settle(now);
        let (id, _) = self.selected?;
        self.selectable(now)
            .into_iter()
            .find(|run| run.id == id)
            .map(|run| run.url.clone())
    }
}

/// Adds the runs of one section to `rows`, or the empty row when there is none.
///
/// `first` is the position of the first run of `runs` among all runs. The run
/// at position `selected` gets the mark.
fn push_runs<'a>(
    rows: &mut Vec<Row<'a>>,
    runs: &[&'a Run],
    section: Section,
    first: usize,
    selected: Option<usize>,
) {
    if runs.is_empty() {
        rows.push(Row::Empty(section));
    }
    rows.extend(runs.iter().enumerate().map(|(offset, &run)| Row::Run {
        run,
        selected: selected == Some(first + offset),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as Span, TimeZone};

    const HOUR: u64 = 3600;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0)
            .single()
            .expect("a valid time")
    }

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    /// Builds a run. The times are minutes before `now()`.
    fn run(id: u64, repo_text: &str, state: RunState, started: i64, updated: i64) -> Run {
        Run {
            id: RunId(id),
            repo: repo(repo_text),
            workflow: "ci".to_owned(),
            branch: "main".to_owned(),
            title: format!("run {id}"),
            state,
            started_at: now() - Span::minutes(started),
            updated_at: now() - Span::minutes(updated),
            url: format!("https://example.test/{repo_text}/runs/{id}"),
        }
    }

    fn board() -> Board {
        Board::new(Duration::from_secs(8 * HOUR))
    }

    /// Writes each row as short text: `Header(Active,2,0,0)`, `Empty(Recent)`,
    /// or `Run(7)`. The selection mark is left out, see `marked`.
    fn shape(rows: &[Row<'_>]) -> Vec<String> {
        marked(rows)
            .into_iter()
            .map(|row| row.trim_end_matches('*').to_owned())
            .collect()
    }

    /// Like `shape`, and a selected run reads `Run(7)*`.
    fn marked(rows: &[Row<'_>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Header {
                    section,
                    count,
                    passed,
                    failed,
                } => format!("Header({section:?},{count},{passed},{failed})"),
                Row::Empty(section) => format!("Empty({section:?})"),
                Row::Run { run, selected } => {
                    format!("Run({}){}", run.id.0, if *selected { "*" } else { "" })
                }
            })
            .collect()
    }

    #[test]
    fn an_empty_board_shows_two_empty_sections() {
        let mut board = board();
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,0,0,0)",
                "Empty(Recent)"
            ]
        );
    }

    #[test]
    fn the_sections_hold_their_runs_and_the_headers_count_them() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 5, 1),
                run(3, "a/x", RunState::Failed, 40, 10),
                run(4, "a/x", RunState::Passed, 50, 15),
                run(2, "a/x", RunState::Passed, 30, 20),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            shape(&rows),
            [
                "Header(Active,1,0,0)",
                "Run(1)",
                "Header(Recent,3,2,1)",
                "Run(3)",
                "Run(4)",
                "Run(2)",
            ]
        );
    }

    #[test]
    fn an_empty_section_shows_an_empty_row_after_its_header() {
        let mut only_active = board();
        only_active.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        let rows = only_active.rows(now());
        assert_eq!(shape(&rows)[2..], ["Header(Recent,0,0,0)", "Empty(Recent)"]);

        let mut only_recent = board();
        only_recent.replace(repo("a/x"), vec![run(2, "a/x", RunState::Passed, 30, 20)]);
        let rows = only_recent.rows(now());
        assert_eq!(shape(&rows)[..2], ["Header(Active,0,0,0)", "Empty(Active)"]);
    }

    #[test]
    fn queued_and_running_runs_are_active_and_other_runs_never_show() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Queued, 20, 20),
                run(2, "a/x", RunState::Running, 10, 5),
                run(3, "a/x", RunState::Other, 30, 25),
                run(4, "a/x", RunState::Passed, 40, 30),
            ],
        );
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,1,1,0)",
                "Run(4)",
            ]
        );
    }

    #[test]
    fn active_runs_are_ordered_oldest_first_and_ties_by_id() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(9, "a/x", RunState::Running, 5, 1),
                run(8, "a/x", RunState::Queued, 30, 1),
                run(7, "a/x", RunState::Running, 10, 1),
                run(5, "a/x", RunState::Running, 10, 1),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(shape(&rows)[1..5], ["Run(8)", "Run(5)", "Run(7)", "Run(9)"]);
    }

    #[test]
    fn recent_runs_are_ordered_newest_first_and_ties_by_id_descending() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Passed, 100, 50),
                run(2, "a/x", RunState::Failed, 100, 10),
                run(3, "a/x", RunState::Passed, 100, 30),
                run(4, "a/x", RunState::Passed, 100, 30),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(shape(&rows)[3..], ["Run(2)", "Run(4)", "Run(3)", "Run(1)"]);
    }

    /// Makes a run that finished `seconds` before `now()`.
    fn finished_run(id: u64, state: RunState, started_secs: i64, finished_secs: i64) -> Run {
        let mut made = run(id, "a/x", state, 0, 0);
        made.started_at = now() - Span::seconds(started_secs);
        made.updated_at = now() - Span::seconds(finished_secs);
        made
    }

    #[test]
    fn a_finished_run_shows_while_it_ended_inside_the_window() {
        let window = 8 * 3600;
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                finished_run(1, RunState::Passed, window + 100, window - 1),
                finished_run(2, RunState::Passed, window + 100, window + 1),
                finished_run(3, RunState::Failed, window + 100, window),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            shape(&rows),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,2,1,1)",
                "Run(1)",
                "Run(3)",
            ]
        );
    }

    #[test]
    fn a_run_that_started_before_the_window_and_ended_inside_it_shows() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![finished_run(1, RunState::Passed, 9 * 3600, 3600)],
        );
        assert_eq!(shape(&board.rows(now()))[3..], ["Run(1)"]);
    }

    #[test]
    fn active_runs_show_whatever_the_window_is() {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                finished_run(1, RunState::Running, 3 * 24 * 3600, 3 * 24 * 3600),
                finished_run(2, RunState::Queued, 20 * 3600, 20 * 3600),
            ],
        );
        assert_eq!(
            shape(&board.rows(now()))[..4],
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,0,0,0)",
            ]
        );
    }

    #[test]
    fn replace_changes_only_the_runs_of_its_repository() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.replace(repo("b/y"), vec![run(2, "b/y", RunState::Running, 4, 1)]);
        assert_eq!(shape(&board.rows(now()))[1..3], ["Run(1)", "Run(2)"]);

        board.replace(
            repo("a/x"),
            vec![
                run(3, "a/x", RunState::Running, 6, 1),
                run(4, "a/x", RunState::Passed, 9, 8),
            ],
        );
        let rows = board.rows(now());
        let shown = shape(&rows);
        assert_eq!(shown[1], "Run(3)");
        assert_eq!(shown[2], "Run(2)");
        assert_eq!(shown[3..], ["Header(Recent,1,1,0)", "Run(4)"]);
        assert_eq!(shown.len(), 5);
    }

    #[test]
    fn a_failed_repository_keeps_its_last_good_runs_and_lists_its_error() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.fail(repo("b/y"), "HTTP 404".to_owned());
        board.fail(repo("a/x"), "timed out".to_owned());
        board.fail(repo("a/w"), "HTTP 500".to_owned());

        assert_eq!(shape(&board.rows(now()))[1], "Run(1)");
        let errors = board.errors();
        let listed: Vec<(String, &str)> = errors
            .iter()
            .map(|(repo, error)| (repo.to_string(), *error))
            .collect();
        assert_eq!(
            listed,
            [
                ("a/w".to_owned(), "HTTP 500"),
                ("a/x".to_owned(), "timed out"),
                ("b/y".to_owned(), "HTTP 404"),
            ]
        );
    }

    #[test]
    fn a_later_replace_clears_the_error_of_its_repository() {
        let mut board = board();
        board.fail(repo("a/x"), "timed out".to_owned());
        board.fail(repo("b/y"), "HTTP 404".to_owned());
        board.replace(repo("a/x"), Vec::new());
        let errors = board.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0.to_string(), "b/y");
    }

    #[test]
    fn a_repository_that_never_had_runs_can_fail_and_shows_no_run() {
        let mut board = board();
        board.fail(repo("a/x"), "HTTP 404".to_owned());
        assert_eq!(
            shape(&board.rows(now())),
            [
                "Header(Active,0,0,0)",
                "Empty(Active)",
                "Header(Recent,0,0,0)",
                "Empty(Recent)"
            ]
        );
        assert_eq!(board.errors().len(), 1);
    }

    #[test]
    fn keep_only_drops_the_runs_and_errors_of_other_repositories() {
        let mut board = board();
        board.replace(repo("a/x"), vec![run(1, "a/x", RunState::Running, 5, 1)]);
        board.replace(repo("b/y"), vec![run(2, "b/y", RunState::Running, 4, 1)]);
        board.fail(repo("b/y"), "timed out".to_owned());
        board.fail(repo("c/z"), "HTTP 404".to_owned());
        board.fail(repo("a/x"), "HTTP 500".to_owned());

        let keep: BTreeSet<RepoName> = [repo("A/X")].into_iter().collect();
        board.keep_only(&keep);

        assert_eq!(
            shape(&board.rows(now()))[..3],
            ["Header(Active,1,0,0)", "Run(1)", "Header(Recent,0,0,0)"]
        );
        let errors = board.errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].1, "HTTP 500");
    }

    #[test]
    fn the_first_run_is_selected_at_the_start() {
        let mut with_active = board();
        with_active.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 5, 1),
                run(2, "a/x", RunState::Passed, 30, 20),
            ],
        );
        assert_eq!(
            marked(&with_active.rows(now())),
            [
                "Header(Active,1,0,0)",
                "Run(1)*",
                "Header(Recent,1,1,0)",
                "Run(2)"
            ]
        );

        let mut only_recent = board();
        only_recent.replace(
            repo("a/x"),
            vec![
                run(3, "a/x", RunState::Passed, 30, 10),
                run(2, "a/x", RunState::Failed, 30, 20),
            ],
        );
        assert_eq!(
            marked(&only_recent.rows(now()))[2..],
            ["Header(Recent,2,1,1)", "Run(3)*", "Run(2)"]
        );
    }

    #[test]
    fn an_empty_board_has_no_selection() {
        let mut board = board();
        let rows = board.rows(now());
        assert!(marked(&rows).iter().all(|row| !row.ends_with('*')));
        assert_eq!(board.selected_url(now()), None);
    }

    /// The text of the one selected row, or `None` when no row is selected.
    fn selected_text(board: &mut Board) -> Option<String> {
        let rows = board.rows(now());
        let found: Vec<String> = marked(&rows)
            .into_iter()
            .filter(|row| row.ends_with('*'))
            .collect();
        assert!(found.len() <= 1, "more than one selected row: {found:?}");
        found.into_iter().next()
    }

    fn four_runs_in_two_sections() -> Board {
        let mut board = board();
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(3, "a/x", RunState::Passed, 60, 10),
                run(4, "a/x", RunState::Failed, 60, 20),
            ],
        );
        board
    }

    #[test]
    fn select_next_moves_through_one_list_and_stops_at_the_end() {
        let mut board = four_runs_in_two_sections();
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(selected_text(&mut board).expect("a selection"));
            board.select_next(now());
        }
        assert_eq!(
            seen,
            ["Run(1)*", "Run(2)*", "Run(3)*", "Run(4)*", "Run(4)*"]
        );
    }

    #[test]
    fn select_previous_moves_back_through_one_list_and_stops_at_the_start() {
        let mut board = four_runs_in_two_sections();
        for _ in 0..3 {
            board.select_next(now());
        }
        let mut seen = Vec::new();
        for _ in 0..5 {
            seen.push(selected_text(&mut board).expect("a selection"));
            board.select_previous(now());
        }
        assert_eq!(
            seen,
            ["Run(4)*", "Run(3)*", "Run(2)*", "Run(1)*", "Run(1)*"]
        );
    }

    #[test]
    fn the_selection_moves_on_an_empty_board_without_a_selection() {
        let mut board = board();
        board.select_next(now());
        board.select_previous(now());
        assert_eq!(selected_text(&mut board), None);
    }

    #[test]
    fn selected_url_is_the_url_of_the_selected_run() {
        let mut board = four_runs_in_two_sections();
        assert_eq!(
            board.selected_url(now()).as_deref(),
            Some("https://example.test/a/x/runs/1")
        );
        board.select_next(now());
        board.select_next(now());
        assert_eq!(
            board.selected_url(now()).as_deref(),
            Some("https://example.test/a/x/runs/3")
        );
    }

    #[test]
    fn the_selection_stays_on_its_run_when_runs_arrive_above_it() {
        let mut board = four_runs_in_two_sections();
        board.select_next(now());
        board.select_next(now());
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(3)*"));

        board.replace(
            repo("b/y"),
            vec![
                run(5, "b/y", RunState::Running, 90, 1),
                run(6, "b/y", RunState::Passed, 60, 2),
            ],
        );
        let rows = board.rows(now());
        assert_eq!(
            marked(&rows),
            [
                "Header(Active,3,0,0)",
                "Run(5)",
                "Run(1)",
                "Run(2)",
                "Header(Recent,3,2,1)",
                "Run(6)",
                "Run(3)*",
                "Run(4)",
            ]
        );
    }

    fn select_third(board: &mut Board) {
        board.select_next(now());
        board.select_next(now());
        assert_eq!(selected_text(board).as_deref(), Some("Run(3)*"));
    }

    #[test]
    fn when_the_selected_run_is_dropped_the_run_at_its_position_is_selected() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(4, "a/x", RunState::Failed, 60, 20),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(4)*"));
    }

    #[test]
    fn when_the_selected_run_was_the_last_the_new_last_run_is_selected() {
        let mut board = four_runs_in_two_sections();
        for _ in 0..3 {
            board.select_next(now());
        }
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(4)*"));
        board.replace(
            repo("a/x"),
            vec![
                run(1, "a/x", RunState::Running, 30, 1),
                run(2, "a/x", RunState::Queued, 20, 1),
                run(3, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(3)*"));
    }

    #[test]
    fn when_the_selected_run_leaves_the_window_the_nearest_run_is_selected() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        let later = now() + Span::hours(8) + Span::minutes(15);
        let rows = board.rows(later);
        assert_eq!(
            marked(&rows),
            [
                "Header(Active,2,0,0)",
                "Run(1)",
                "Run(2)*",
                "Header(Recent,0,0,0)",
                "Empty(Recent)",
            ]
        );
    }

    #[test]
    fn with_no_run_left_nothing_is_selected_and_the_first_run_is_selected_again_later() {
        let mut board = four_runs_in_two_sections();
        select_third(&mut board);
        board.replace(repo("a/x"), Vec::new());
        assert_eq!(selected_text(&mut board), None);
        assert_eq!(board.selected_url(now()), None);

        board.replace(
            repo("a/x"),
            vec![
                run(7, "a/x", RunState::Running, 30, 1),
                run(8, "a/x", RunState::Passed, 60, 10),
            ],
        );
        assert_eq!(selected_text(&mut board).as_deref(), Some("Run(7)*"));
    }

    #[test]
    fn a_tall_view_shows_all_rows_from_the_first() {
        let mut board = four_runs_in_two_sections();
        let all = marked(&board.rows(now()));
        let view = board.view(now(), 100);
        assert_eq!(view.first, 0);
        assert_eq!(marked(&view.rows), all);
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn a_view_of_height_zero_has_no_rows() {
        let mut board = four_runs_in_two_sections();
        let view = board.view(now(), 0);
        assert!(view.rows.is_empty());
        assert_eq!(view.first, 0);
        let mut empty = Board::new(Duration::from_secs(HOUR));
        assert!(empty.view(now(), 0).rows.is_empty());
    }

    #[test]
    fn a_short_view_shows_the_first_rows() {
        let mut board = four_runs_in_two_sections();
        let view = board.view(now(), 3);
        assert_eq!(view.first, 0);
        assert_eq!(
            marked(&view.rows),
            ["Header(Active,2,0,0)", "Run(1)*", "Run(2)"]
        );
    }
}
