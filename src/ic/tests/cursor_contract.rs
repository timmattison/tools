//! Cursor contract tests for the display routines of `ic`.
//!
//! Sixel gives no contract for the position of the cursor after the string
//! terminator. Each renderer makes its own decision. The Kitty protocol and the
//! iTerm2 protocol both have a flag that holds the cursor still, but then the
//! caller must move it. `ic` must therefore state where the cursor ends, in
//! every routine, instead of a guess of one newline.
//!
//! These tests drive the real binary and look at the bytes it writes. They
//! measure the cursor movement that the stream *requests*, not the exact escape
//! sequences, so a change of spelling keeps them alive.

use std::io::Write;
use std::process;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// The escape byte that starts every escape sequence.
const ESC: u8 = 0x1b;

/// The second byte of a control sequence introducer.
const CSI_BRACKET: u8 = b'[';

/// The final byte of a CUU (cursor up) sequence.
const CURSOR_UP_FINAL: u8 = b'A';

/// The final byte of a CUD (cursor down) sequence.
const CURSOR_DOWN_FINAL: u8 = b'B';

/// The device control string introducer that opens the Sixel payload.
const SIXEL_START: &[u8] = b"\x1bP";

/// The application program command that opens a Kitty graphics command.
const KITTY_START: &[u8] = b"\x1b_G";

/// The operating system command that opens an iTerm2 inline image.
const ITERM2_START: &[u8] = b"\x1b]1337;";

/// The bell that closes an iTerm2 inline image.
const ITERM2_END: &[u8] = b"\x07";

/// The Kitty graphics key that tells the renderer not to move the cursor.
const KITTY_NO_CURSOR_MOVE: &str = "C=1";

/// The string terminator that closes the Sixel payload.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// DECSC. It saves the position of the cursor.
const SAVE_CURSOR: &[u8] = b"\x1b7";

/// DECRC. It restores the saved position of the cursor.
const RESTORE_CURSOR: &[u8] = b"\x1b8";

/// The image that the tests send to `ic` on stdin.
const TEST_IMAGE: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png"));

/// The path of the image that the tests give to `ic` as an argument.
///
/// The file is in the repository and no test writes to it, so two concurrent
/// runs of this file cannot disturb each other.
const TEST_IMAGE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_image.png");

/// The number of rows of the terminal that `ic` sees in these tests.
///
/// `get_terminal_size` falls back to 80 columns by 24 rows when it cannot read
/// the size of the terminal. The tests give `ic` a pipe for stdout, so `ic`
/// always uses this fallback.
const TERMINAL_ROWS: i64 = 24;

/// The row that the shell prompt returns to below the image.
const PROMPT_ROWS: i64 = 1;

/// The terminal type of a child process that must not look like Kitty.
const TERM_XTERM_256COLOR: &str = "xterm-256color";

/// The terminal type that Kitty sets.
const TERM_XTERM_KITTY: &str = "xterm-kitty";

/// A display routine of `ic` that a test selects.
///
/// `ic` picks the routine from the environment, so a test names the routine it
/// wants and [`Routine::environment`] gives the variables that select it.
#[derive(Clone, Copy, Debug)]
enum Routine {
    /// The Sixel routine, which muxiavelli panels and Zellij use.
    Sixel,
    /// The Kitty graphics routine, which Kitty, WezTerm and Ghostty use.
    Kitty,
    /// The iTerm2 inline image routine.
    Iterm2,
}

impl Routine {
    /// Give the environment variables that select this routine.
    ///
    /// The variables go on top of a cleared environment, so nothing that the
    /// test runner inherited can change the routine.
    ///
    /// `MUXIAVELLI` selects the Sixel routine. `ZELLIJ` selects the same
    /// routine but it also turns on a process tree heuristic that reads the
    /// real process list of the host, which makes the result depend on the
    /// machine.
    ///
    /// # Returns
    /// The name and the value of each variable.
    fn environment(self) -> Vec<(&'static str, &'static str)> {
        match self {
            Routine::Sixel => vec![
                ("TERM", TERM_XTERM_256COLOR),
                ("MUXIAVELLI", "1"),
                ("MUXIAVELLI_IMAGE_PROTOCOLS", "sixel"),
            ],
            Routine::Kitty => vec![("TERM", TERM_XTERM_KITTY), ("KITTY_WINDOW_ID", "1")],
            Routine::Iterm2 => vec![("TERM", TERM_XTERM_256COLOR), ("TERM_PROGRAM", "iTerm.app")],
        }
    }

    /// Give the bytes that open the image payload of this routine.
    ///
    /// # Returns
    /// The introducer of the image protocol.
    fn payload_start(self) -> &'static [u8] {
        match self {
            Routine::Sixel => SIXEL_START,
            Routine::Kitty => KITTY_START,
            Routine::Iterm2 => ITERM2_START,
        }
    }

    /// Give the bytes that close the image payload of this routine.
    ///
    /// # Returns
    /// The terminator of the image protocol. The Kitty routine sends a large
    /// image in more than one command, so the *last* terminator of the stream
    /// closes the payload.
    fn payload_end(self) -> &'static [u8] {
        match self {
            Routine::Sixel | Routine::Kitty => STRING_TERMINATOR,
            Routine::Iterm2 => ITERM2_END,
        }
    }
}

/// A directory that does not exist, unique to this process.
///
/// The `PATH` of the child points here, which keeps `ps` out of reach. The
/// remote transport detection then finds no process tree to walk, so a test
/// runner that is itself under a remote transport cannot change the bytes that
/// `ic` writes. The directory holds the process id and a nanosecond stamp, so
/// two concurrent runs of this file never name the same directory.
///
/// # Returns
/// The path of a directory that no process creates.
fn unreachable_path_dir() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock must be after the epoch")
        .as_nanos();
    format!("/nonexistent-ic-cursor-contract-{}-{nanos}", process::id())
}

/// The number of terminal rows that the test image occupies.
///
/// The test invocation makes a Sixel image of 100 pixels by 100 pixels. One
/// character cell is 20 pixels high, so the image fills 5 rows. The Kitty
/// routine and the iTerm2 routine take the size of the image in character
/// cells, and the same test invocation asks them for 5 rows.
const EXPECTED_ROWS: i64 = 5;

/// The cursor movement that a byte stream requests, in rows.
struct CursorMovement {
    /// The total number of rows of downward movement.
    down: i64,
    /// The total number of rows of upward movement.
    up: i64,
}

impl CursorMovement {
    /// Give the net movement in rows. A positive result is downward.
    fn net(&self) -> i64 {
        self.down - self.up
    }
}

/// Scan a byte slice and measure the cursor movement that it requests.
///
/// A newline moves the cursor down one row. A CUD sequence (`ESC [ n B`) moves
/// the cursor down `n` rows and a CUU sequence (`ESC [ n A`) moves it up `n`
/// rows. A missing or zero parameter means one row, which is what a terminal
/// does. The scan ignores all other bytes.
///
/// # Arguments
/// * `bytes` - The byte stream to scan.
///
/// # Returns
/// The total downward and upward movement in rows.
fn scan_cursor_movement(bytes: &[u8]) -> CursorMovement {
    let mut movement = CursorMovement { down: 0, up: 0 };
    let mut index = 0_usize;

    while index < bytes.len() {
        if bytes[index] == b'\n' {
            movement.down += 1;
            index += 1;
            continue;
        }

        if bytes[index] != ESC || bytes.get(index + 1) != Some(&CSI_BRACKET) {
            index += 1;
            continue;
        }

        let mut end = index + 2;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }

        let parameter: i64 = std::str::from_utf8(&bytes[index + 2..end])
            .ok()
            .and_then(|digits| digits.parse().ok())
            .unwrap_or(0);
        // A missing or zero parameter means one row.
        let rows = if parameter == 0 { 1 } else { parameter };

        match bytes.get(end) {
            Some(&CURSOR_DOWN_FINAL) => movement.down += rows,
            Some(&CURSOR_UP_FINAL) => movement.up += rows,
            _ => {}
        }

        index = end.saturating_add(1).min(bytes.len());
    }

    movement
}

/// Find the first position of a byte pattern in a byte slice.
///
/// # Arguments
/// * `haystack` - The byte slice to search.
/// * `needle` - The byte pattern to look for.
///
/// # Returns
/// The index of the first byte of the match, or `None` when there is no match.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Find the last position of a byte pattern in a byte slice.
///
/// # Arguments
/// * `haystack` - The byte slice to search.
/// * `needle` - The byte pattern to look for.
///
/// # Returns
/// The index of the first byte of the last match, or `None` when there is no
/// match.
fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// Check that a byte stream keeps the cursor contract of `ic`.
///
/// The contract has three parts:
///
/// 1. The stream reserves the rows of the image before the payload and then
///    takes them back, so an image at the bottom of the screen scrolls the
///    terminal instead of running off it.
/// 2. DECSC comes immediately before the payload and DECRC immediately after
///    it, so cursor motion inside the payload cannot change the final position
///    of the cursor.
/// 3. The bytes after the payload move the cursor down by the row count of the
///    image and put it at column 1.
///
/// # Arguments
/// * `routine` - The display routine that wrote the stream.
/// * `stdout` - The bytes that `ic` wrote to stdout.
///
/// # Panics
/// Panics when the stream breaks any part of the contract.
fn assert_cursor_contract(routine: Routine, stdout: &[u8]) {
    let payload_start = find(stdout, routine.payload_start())
        .unwrap_or_else(|| panic!("the output of {routine:?} must hold an image payload"));
    let payload_end = rfind(stdout, routine.payload_end())
        .unwrap_or_else(|| panic!("the output of {routine:?} must close its image payload"))
        + routine.payload_end().len();

    // Part 1. The reservation runs from the start of the stream to DECSC.
    assert!(
        payload_start >= SAVE_CURSOR.len(),
        "the stream must save the cursor before the payload"
    );
    let save = payload_start - SAVE_CURSOR.len();
    let reservation = scan_cursor_movement(&stdout[..save]);
    assert_eq!(
        reservation.down, EXPECTED_ROWS,
        "the reservation must ask for {EXPECTED_ROWS} rows before the payload"
    );
    assert_eq!(
        reservation.net(),
        0,
        "the reservation must take the rows back, so the image starts at the top of them"
    );

    // Part 2.
    assert_eq!(
        &stdout[save..payload_start],
        SAVE_CURSOR,
        "DECSC must come immediately before the payload"
    );
    assert!(
        stdout.len() >= payload_end + RESTORE_CURSOR.len(),
        "the stream must restore the cursor after the payload"
    );
    assert_eq!(
        &stdout[payload_end..payload_end + RESTORE_CURSOR.len()],
        RESTORE_CURSOR,
        "DECRC must come immediately after the payload"
    );

    // Part 3.
    let tail = &stdout[payload_end..];
    assert_eq!(
        scan_cursor_movement(tail).net(),
        EXPECTED_ROWS,
        "the bytes after the payload must move the cursor down {EXPECTED_ROWS} rows"
    );
    assert!(
        tail.ends_with(b"\r") || tail.ends_with(b"\n"),
        "the stream must end the line, to put the cursor at column 1"
    );
}

/// Give the key list of the first Kitty graphics command in a byte stream.
///
/// A Kitty graphics command is `ESC _ G <key list> ; <payload> ESC \`. The key
/// list is a comma separated list of `<key>=<value>` pairs.
///
/// # Arguments
/// * `bytes` - The byte stream to read.
///
/// # Returns
/// One entry for each `<key>=<value>` pair of the first graphics command.
///
/// # Panics
/// Panics when the stream holds no Kitty graphics command, or when the command
/// holds no key list.
fn kitty_key_list(bytes: &[u8]) -> Vec<String> {
    let start = find(bytes, KITTY_START).expect("the output must hold a Kitty graphics command")
        + KITTY_START.len();
    let tail = &bytes[start..];
    let end = tail
        .iter()
        .position(|byte| *byte == b';')
        .expect("a Kitty graphics command must close its key list with a semicolon");

    std::str::from_utf8(&tail[..end])
        .expect("a Kitty key list must be UTF-8")
        .split(',')
        .map(str::to_owned)
        .collect()
}

/// Make a command that runs `ic` through one display routine.
///
/// # Arguments
/// * `routine` - The display routine that `ic` must use.
/// * `args` - The full command line for `ic`.
///
/// # Returns
/// A command with the environment and the pipes of the tests already set.
fn ic_command(routine: Routine, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ic"));
    command
        .args(args)
        .env_clear()
        .env("PATH", unreachable_path_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (name, value) in routine.environment() {
        command.env(name, value);
    }

    command
}

/// Wait for `ic` to exit and give back its stdout.
///
/// # Arguments
/// * `child` - The child process to wait for.
///
/// # Returns
/// The bytes that `ic` wrote to stdout.
///
/// # Panics
/// Panics when the wait fails or when `ic` exits with a failure.
fn wait_for_stdout(child: process::Child) -> Vec<u8> {
    let output = child.wait_with_output().expect("failed to wait for ic");
    assert!(
        output.status.success(),
        "ic exited with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    output.stdout
}

/// Run `ic` through one display routine, with the image on stdin, and give back
/// its stdout.
///
/// # Arguments
/// * `routine` - The display routine that `ic` must use.
/// * `extra_args` - More command line arguments for `ic`, after the arguments
///   that fix the size of the image.
///
/// # Returns
/// The bytes that `ic` wrote to stdout.
///
/// # Panics
/// Panics when the child process does not start, does not accept the image, or
/// exits with a failure.
fn run_ic(routine: Routine, extra_args: &[&str]) -> Vec<u8> {
    let mut args: Vec<&str> = vec!["--stdin", "--width", "10", "--height", "5"];
    args.extend_from_slice(extra_args);

    let mut child = ic_command(routine, &args)
        .spawn()
        .expect("failed to start ic");

    let mut stdin = child.stdin.take().expect("ic has no stdin pipe");
    stdin
        .write_all(TEST_IMAGE)
        .expect("failed to send the image to ic");
    drop(stdin);

    wait_for_stdout(child)
}

/// Run `ic` on an image file through the Sixel display routine, with no size
/// arguments, and give back its stdout. The missing size arguments select the
/// auto-fit path.
///
/// # Arguments
/// * `path` - The path of the image file.
///
/// # Returns
/// The bytes that `ic` wrote to stdout.
///
/// # Panics
/// Panics when the child process does not start or exits with a failure.
fn run_ic_sixel_auto_fit(path: &str) -> Vec<u8> {
    let child = ic_command(Routine::Sixel, &[path])
        .spawn()
        .expect("failed to start ic");

    wait_for_stdout(child)
}

/// The bytes after the Sixel string terminator must move the cursor down by the
/// row count of the image, and must put it at column 1. This implementation
/// ends the line with a carriage return.
#[test]
fn sixel_advances_the_cursor_below_the_image() {
    let stdout = run_ic(Routine::Sixel, &[]);

    let terminator =
        find(&stdout, STRING_TERMINATOR).expect("the output must hold a Sixel string terminator");
    let tail = &stdout[terminator + STRING_TERMINATOR.len()..];

    assert_eq!(
        scan_cursor_movement(tail).net(),
        EXPECTED_ROWS,
        "the bytes after the payload must move the cursor down {EXPECTED_ROWS} rows"
    );
    assert!(
        tail.ends_with(b"\r") || tail.ends_with(b"\n"),
        "the stream must end the line, to put the cursor at column 1"
    );
}

/// DECSC must come immediately before the payload and DECRC immediately after
/// it. The brackets make sure that cursor motion inside the payload cannot
/// change the final position of the cursor.
#[test]
fn sixel_brackets_the_payload_against_renderer_cursor_motion() {
    let stdout = run_ic(Routine::Sixel, &[]);

    let payload_start = find(&stdout, SIXEL_START).expect("the output must hold a Sixel payload");
    let terminator =
        find(&stdout, STRING_TERMINATOR).expect("the output must hold a Sixel string terminator");

    assert!(
        payload_start >= SAVE_CURSOR.len(),
        "the stream must save the cursor before the payload"
    );
    assert_eq!(
        &stdout[payload_start - SAVE_CURSOR.len()..payload_start],
        SAVE_CURSOR,
        "DECSC must come immediately before the payload"
    );

    let payload_end = terminator + STRING_TERMINATOR.len();
    assert!(
        stdout.len() >= payload_end + RESTORE_CURSOR.len(),
        "the stream must restore the cursor after the payload"
    );
    assert_eq!(
        &stdout[payload_end..payload_end + RESTORE_CURSOR.len()],
        RESTORE_CURSOR,
        "DECRC must come immediately after the payload"
    );
}

/// `--no-newline` must suppress the whole cursor contract, because video
/// playback puts the cursor where it wants it before every frame. A stream that
/// moves the cursor on its own scrolls the terminal once per frame.
///
/// The behavior already held when this test arrived, so a mutation proved that
/// the test can fail: with the advance moved outside the `no_newline` gate, the
/// tail asks for 5 rows and the whole stream nets 5 rows, and both assertions
/// below report the failure.
#[test]
fn no_newline_suppresses_the_cursor_contract() {
    let stdout = run_ic(Routine::Sixel, &["--no-newline"]);

    let terminator =
        find(&stdout, STRING_TERMINATOR).expect("the output must hold a Sixel string terminator");
    let tail = &stdout[terminator + STRING_TERMINATOR.len()..];
    let after_payload = scan_cursor_movement(tail);

    assert_eq!(
        (after_payload.down, after_payload.up),
        (0, 0),
        "--no-newline must write no cursor movement after the payload"
    );
    assert_eq!(
        scan_cursor_movement(&stdout).net(),
        0,
        "--no-newline must leave the cursor where the caller put it"
    );
}

/// The stream must reserve the rows of the image before it draws. It asks for
/// the rows and then takes them back, so an image at the bottom of the screen
/// scrolls the terminal instead of running off it.
#[test]
fn sixel_reserves_the_rows_before_it_draws() {
    let stdout = run_ic(Routine::Sixel, &[]);

    let save = find(&stdout, SAVE_CURSOR).expect("the stream must save the cursor");
    let payload_start = find(&stdout, SIXEL_START).expect("the output must hold a Sixel payload");
    assert!(
        save < payload_start,
        "the reservation must come before the payload"
    );

    let reservation = &stdout[..save];
    let movement = scan_cursor_movement(reservation);
    assert_eq!(
        movement.down, EXPECTED_ROWS,
        "the reservation must ask for {EXPECTED_ROWS} rows before the payload"
    );
    assert_eq!(
        movement.net(),
        0,
        "the reservation must take the rows back, so the image starts at the top of them"
    );
}

/// The file name row, the image and the prompt must fit inside the height of
/// the terminal. `ic` prints the file name above the image, so the auto-fit
/// path must pay for that row.
///
/// The net movement of the whole stream is the number of rows that the terminal
/// advances: the header rows plus the image rows, because the reservation and
/// the cursor-up cancel. The prompt then takes one more row.
#[test]
fn auto_fit_leaves_room_for_the_file_name_and_the_prompt() {
    let stdout = run_ic_sixel_auto_fit(TEST_IMAGE_PATH);

    let rows = scan_cursor_movement(&stdout).net();
    assert!(
        rows + PROMPT_ROWS <= TERMINAL_ROWS,
        "the file name row and the image take {rows} rows, and the prompt takes {PROMPT_ROWS} more, which is more than the {TERMINAL_ROWS} rows of the terminal"
    );
}

/// The Kitty stream must keep the same cursor contract as the Sixel stream. It
/// reserves the rows of the image and takes them back, it brackets the payload
/// with DECSC and DECRC, and it then moves the cursor down by the row count and
/// puts it at column 1.
#[test]
fn kitty_meets_the_cursor_contract() {
    assert_cursor_contract(Routine::Kitty, &run_ic(Routine::Kitty, &[]));
}

/// The Kitty graphics command must carry `C=1`, which tells the renderer not to
/// move the cursor.
///
/// The routine states the position of the cursor itself. A renderer that also
/// moves the cursor doubles the movement, so the routine must hold it still.
#[test]
fn kitty_holds_the_cursor_still_while_it_draws() {
    let stdout = run_ic(Routine::Kitty, &[]);

    let keys = kitty_key_list(&stdout);
    assert!(
        keys.iter().any(|key| key == KITTY_NO_CURSOR_MOVE),
        "the Kitty key list must carry {KITTY_NO_CURSOR_MOVE}, but it is {keys:?}"
    );
}

/// The iTerm2 stream must keep the same cursor contract as the other two
/// routines. It already tells the renderer not to move the cursor with
/// `doNotMoveCursor=1`, so it owes the caller the movement itself.
#[test]
fn iterm2_meets_the_cursor_contract() {
    assert_cursor_contract(Routine::Iterm2, &run_ic(Routine::Iterm2, &[]));
}
