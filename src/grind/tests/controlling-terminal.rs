//! Black-box tests that say where the width of the breakdown comes from.
//!
//! `grind` lines the hunk counts of its breakdown up in one column, and past
//! the right-hand edge of the terminal there is no such column. So the layout
//! takes a width, and this file holds `grind` to where that width comes from.
//!
//! Two sources answer, and they disagree. The first source is the terminal that
//! the run holds, which `grind` measures through `/dev/tty`. The second source
//! is the width that the environment states in `COLUMNS`, which a wrapper such
//! as `viddy(1)` exports and which a caller sets by hand. The rule is that a
//! statement wins: a run that states a width is laid out for that width, and a
//! run that states none is laid out for the terminal it holds.
//!
//! The rule needs a test that holds a terminal, and `cargo test` gives the test
//! binary the terminal of whoever started it. A test that read that terminal
//! would answer one way in a wide window and another way in a narrow one, which
//! is the defect this file exists to keep out. So each test below opens a
//! pseudo-terminal of a size it chose, gives it to the child as the controlling
//! terminal, and gives the child a pipe for standard output. The child then
//! holds a terminal that `grind` can measure and a standard output that
//! measures nothing, which is the shape of every captured run.
//!
//! A pseudo-terminal that nobody sized reports zero columns, and the
//! `TIOCGWINSZ` ioctl succeeds on it. Every terminal here therefore arrives
//! sized, and the size arrives with the `openpty` call so that no window of the
//! wrong size ever exists.

#![cfg(unix)]

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::ptr;

use gitscratch::testing::nested_conflict_repo;
use gitscratch::NoInheritedGitEnvironment;

/// The branch that [`nested_conflict_repo`] stands on for every run here.
const HEAD_BRANCH: &str = "left";

/// The branch that every run here replays HEAD onto.
const ONTO_BRANCH: &str = "right";

/// The variable that states how many columns wide the terminal is.
const WIDTH_VARIABLE: &str = "COLUMNS";

/// The locale every run here is pinned to, for the reason `cli.rs` states.
const PINNED_LOCALE: [(&str, &str); 2] = [("LC_ALL", "C"), ("LANG", "C")];

/// The number of rows of every pseudo-terminal here.
///
/// The layout reads the columns and no row, so this number reaches nothing that
/// the tests assert. It is above zero because a terminal of zero rows carries
/// no window, and `grind` refuses such a terminal and falls back.
const TERMINAL_ROWS: u16 = 24;

/// A width too narrow to hold the longer name of [`nested_conflict_repo`]
/// beside its count.
///
/// The two names are `shared.txt` at 10 columns and `sub/nested/shared.txt` at
/// 21, and the widest count is `1 hunk` at 6. The layout keeps 2 columns of
/// indent and 4 columns of gap, so the longer name needs 33 columns and this
/// width leaves it 18. The shorter name still fits beside its count.
const NARROW: u16 = 30;

/// A width wide enough to hold both names beside their counts.
///
/// Each test below pairs this width with [`NARROW`], one as the terminal and
/// one as the statement. Two widths over one fixture is what says which of the
/// two sources the layout came from. One width alone says only that the output
/// has a shape.
const WIDE: u16 = 200;

/// The breakdown of [`nested_conflict_repo`] laid out for [`NARROW`] columns.
///
/// The longer name takes a row of its own and its count takes the next row, in
/// the column the shorter name's count already stands in. The name itself is
/// never cut short, because a truncated path opens no file. Every row is 30
/// columns or fewer, which is what the clamp is for.
const CLAMPED: [&str; 3] = [
    "  shared.txt            1 hunk",
    "  sub/nested/shared.txt",
    "                        1 hunk",
];

/// The breakdown of [`nested_conflict_repo`] laid out for [`WIDE`] columns.
///
/// Both names fit beside their counts, so the clamp changes nothing and the two
/// counts share one column.
const UNCLAMPED: [&str; 2] = [
    "  shared.txt               1 hunk",
    "  sub/nested/shared.txt    1 hunk",
];

/// A pseudo-terminal of a size that a test chose.
///
/// Both ends stay open for the life of the child. The master end holds the
/// pseudo-terminal alive, and the slave end is the terminal that the child
/// takes as its own.
struct Pty {
    /// The master end. The parent holds it open and reads nothing from it,
    /// because the child writes its verdict to a pipe and not to the terminal.
    master: libc::c_int,
    /// The slave end. It becomes the controlling terminal of the child.
    slave: libc::c_int,
}

impl Pty {
    /// Open a pseudo-terminal `columns` columns wide.
    ///
    /// # Returns
    /// The two ends of a pseudo-terminal that reports `columns` columns by
    /// [`TERMINAL_ROWS`] rows.
    ///
    /// # Panics
    /// Panics when the system opens no pseudo-terminal.
    fn open(columns: u16) -> Self {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut size = libc::winsize {
            ws_row: TERMINAL_ROWS,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: `openpty` writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The two null
        // pointers are the documented way to ask for the default terminal modes
        // and to ask for no name of the slave device. The last pointer is the
        // size of the window, and it points at a live local variable that
        // outlives the call.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(
            result,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        Pty { master, slave }
    }
}

impl Drop for Pty {
    /// Close both ends of the pseudo-terminal.
    ///
    /// A test that leaks a file descriptor for each run empties the table of
    /// the process, and the runs of this file share one process.
    fn drop(&mut self) {
        // SAFETY: each descriptor came from the one `openpty` call of
        // [`Pty::open`], nothing else closes them, and `Drop` runs one time.
        unsafe {
            libc::close(self.slave);
            libc::close(self.master);
        }
    }
}

/// Run `grind` in `repo` under a terminal `terminal_columns` wide, stating
/// `stated_columns` when the caller names one.
///
/// The child starts a session of its own and then claims the slave end of the
/// pseudo-terminal as its controlling terminal. `/dev/tty` in the child
/// therefore resolves to that pseudo-terminal, while standard output stays a
/// pipe.
///
/// `None` for `stated_columns` takes the variable away rather than leaving it
/// alone, because the shell of whoever runs the suite can export one. A test of
/// what a run does with no statement must hold no statement.
///
/// # Panics
/// Panics when the child does not start.
fn grind_within(repo: &Path, terminal_columns: u16, stated_columns: Option<u16>) -> Output {
    let pty = Pty::open(terminal_columns);

    let mut command = Command::new(env!("CARGO_BIN_EXE_grind"));
    command
        .arg(ONTO_BRANCH)
        .current_dir(repo)
        .without_inherited_git_environment();

    // After the scrub, for the reason `cli.rs` gives: the rule the scrub
    // applies is the `GIT_` prefix, and no name below wears it.
    command.envs(PINNED_LOCALE);
    match stated_columns {
        Some(columns) => command.env(WIDTH_VARIABLE, columns.to_string()),
        None => command.env_remove(WIDTH_VARIABLE),
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let slave = pty.slave;
    // SAFETY: the closure runs in the child between the fork and the exec, and
    // it calls two functions. `setsid` and `ioctl` are both async-signal-safe,
    // and neither one touches memory of this process: the ioctl takes the
    // request `TIOCSCTTY`, which reads no pointer. The child is never a process
    // group leader in that window, because the fork gave it a new process id
    // and the process group is still the one of the parent, so the one
    // documented failure of `setsid` cannot happen.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }

            #[allow(
                clippy::disallowed_methods,
                reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of the child, and termsize offers no call for that"
            )]
            if libc::ioctl(slave, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }

            Ok(())
        });
    }

    // The pseudo-terminal lives until this function returns, so it is still the
    // terminal of the child for the whole run.
    command.output().expect("failed to run grind")
}

/// The breakdown lines of `output`, which are everything after the blank line
/// that separates them from the summary.
///
/// # Panics
/// Panics when `grind` wrote anything but UTF-8 to standard output.
fn breakdown(output: &Output) -> Vec<String> {
    let stdout = std::str::from_utf8(&output.stdout).expect("grind must write text");

    stdout
        .lines()
        .skip_while(|line| !line.is_empty())
        .skip(1)
        .map(ToOwned::to_owned)
        .collect()
}

/// Everything the run left on standard error, for an assertion that fails to
/// print the whole picture.
///
/// # Panics
/// Panics when `grind` wrote anything but UTF-8 to standard error.
fn complaints(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("grind must write text")
}

/// A run that states no width is laid out for the terminal it holds.
///
/// This is the source that answers when nothing else does, and it is the half
/// of the rule that a fix for the other half can quietly undo. A `grind` that
/// took the fallback of 80 columns whatever terminal it held would pass every
/// other test in this file and lay a nested path out past the right-hand edge
/// of a small window, which is the defect the clamp exists to close.
#[test]
fn a_run_that_states_no_width_is_laid_out_for_the_terminal_it_holds() {
    let repo = nested_conflict_repo();
    repo.checkout(HEAD_BRANCH);

    let output = grind_within(repo.path(), NARROW, None);

    assert_eq!(
        breakdown(&output),
        CLAMPED,
        "a terminal of {NARROW} columns is the only width this run holds, so \
         the breakdown must be laid out for it\nstderr:\n{}",
        complaints(&output)
    );
}

/// A stated width beats a terminal narrower than it.
///
/// The pair below is the shape of a wrapper: the wrapper owns the terminal and
/// hands the tool a pipe, so the tool measures a window that is not the one its
/// text lands in. The wrapper states the real width in `COLUMNS`, and a tool
/// that measures instead of reading the statement lays its rows out for the
/// wrong window.
#[test]
fn a_stated_width_beats_a_narrower_terminal() {
    let repo = nested_conflict_repo();
    repo.checkout(HEAD_BRANCH);

    let output = grind_within(repo.path(), NARROW, Some(WIDE));

    assert_eq!(
        breakdown(&output),
        UNCLAMPED,
        "the run states {WIDE} columns and holds a terminal of {NARROW}; the \
         statement is the width the layout must take\nstderr:\n{}",
        complaints(&output)
    );
}

/// A stated width beats a terminal wider than it.
///
/// The other direction of the same claim, and the one that keeps a test suite
/// honest. Every golden in `cli.rs` is a breakdown, and every one of them is
/// read back from a child of `cargo test` that holds the terminal of whoever
/// started the run. A `grind` that measured that terminal would answer one way
/// in a wide window and another way in a narrow one, and no line of the suite
/// would say which window it was written in.
///
/// Both directions together also foreclose the two rules that satisfy one
/// direction alone: taking the narrower of the two sources, and taking the
/// wider.
#[test]
fn a_stated_width_beats_a_wider_terminal() {
    let repo = nested_conflict_repo();
    repo.checkout(HEAD_BRANCH);

    let output = grind_within(repo.path(), WIDE, Some(NARROW));

    assert_eq!(
        breakdown(&output),
        CLAMPED,
        "the run states {NARROW} columns and holds a terminal of {WIDE}; the \
         statement is the width the layout must take\nstderr:\n{}",
        complaints(&output)
    );
}
