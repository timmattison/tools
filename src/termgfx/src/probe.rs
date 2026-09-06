//! The question this crate asks the terminal, and what it makes of the answer.
//!
//! The rest of this crate reads the environment and names the terminal from
//! what it finds. That answer is a guess, and it is wrong in the one place a
//! guess costs the most: a terminal that carries no name of its own. A pane of
//! a multiplexer and a session of mosh both arrive that way, and both of them
//! draw pictures.
//!
//! A terminal answers two questions about the sequences it reads. The kitty
//! graphics protocol carries a query action, and a terminal that draws it
//! answers `OK`. The primary device attributes carry parameter 4, which names
//! sixel. So the sentence this crate was written on, "a terminal answers no
//! question about the ones it reads", is false for two of the three protocols.
//!
//! # Why the answer is worth a round trip
//!
//! mosh is the case that pays for it. A mosh session runs the emulator on the
//! server, and that emulator answers both questions for the **pair** — mosh
//! together with the terminal of the user. So a program inside a mosh session
//! learns what the far terminal draws, and it learns it from the one party
//! that knows. No environment variable carries that answer, because no
//! variable crosses the session.
//!
//! # The shape of the round trip
//!
//! [`ask_the_terminal`] writes [`IMAGE_QUERY`] to the controlling terminal and
//! reads until the answer of the primary device attributes arrives or until
//! the budget is spent. **The attributes request stands last, and it must stay
//! last.** Every terminal answers it, so its answer is what ends the read. A
//! terminal that draws no kitty graphics answers the first question with
//! silence, and a read that waited for that silence would spend the whole
//! budget on every run.
//!
//! # The run that owns no terminal
//!
//! The question needs raw mode, and the call that asks for raw mode sends
//! SIGTTOU to a caller that stands in a background process group. The default
//! action of SIGTTOU stops the process. So the probe asks
//! [`owns_the_terminal`] first, and a run that owns the terminal is the only
//! run that asks it anything.
//!
//! # The answer that arrives behind the budget
//!
//! The budget ends the read, and it ends no answer of a terminal. A terminal
//! that answers late writes those bytes to the descriptor the shell of the
//! user reads next, and the shell takes them for keystrokes. So the budget is
//! generous enough that a terminal on the far side of a network reaches it,
//! and the probe runs only for a terminal that carries no name at all.

use crate::detect::AnsweredProtocol;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

/// The bytes [`ask_the_terminal`] writes.
///
/// The first is the query action of the kitty graphics protocol: a
/// transmission of one pixel that the terminal answers and never draws. The
/// second is the request of the primary device attributes, which every
/// terminal answers and which therefore ends the read.
pub(crate) const IMAGE_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";

/// How long [`ask_the_terminal`] waits for the answer.
pub(crate) const QUERY_BUDGET: Duration = Duration::from_millis(500);

/// The protocol the answer of a terminal names, or [`None`] for an answer that
/// names neither.
///
/// A kitty answer outranks a sixel one. A terminal that draws both draws a
/// kitty transmission with no palette and no band, so the picture keeps every
/// colour it arrived with.
pub(crate) fn read_answer(answer: &[u8]) -> Option<AnsweredProtocol> {
    if kitty_said_ok(answer) {
        Some(AnsweredProtocol::Kitty)
    } else if attributes_name_sixel(answer) {
        Some(AnsweredProtocol::Sixel)
    } else {
        None
    }
}

/// The opener of an application-program command, which carries a kitty answer.
const APC_OPENER: &[u8] = b"\x1b_";

/// The string terminator that ends one.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// The opener of an answer of the primary device attributes.
const ATTRIBUTES_OPENER: &[u8] = b"\x1b[?";

/// The final byte of an answer of the primary device attributes.
const ATTRIBUTES_FINAL: u8 = b'c';

/// The parameter of that answer which names sixel.
const SIXEL_PARAMETER: &[u8] = b"4";

/// The message a terminal writes for a query it carried out.
const KITTY_OK: &[u8] = b"OK";

/// The key that opens the control block of a graphics answer.
const KITTY_GRAPHICS: u8 = b'G';

/// The byte that divides the control block of a kitty answer from its message.
const KITTY_SEPARATOR: u8 = b';';

/// Whether the terminal carried out the query of [`IMAGE_QUERY`].
///
/// The answer is `ESC _ G <keys> ; <message> ESC \`, and the message is `OK`
/// for a query the terminal carried out. A terminal that refused it writes a
/// code such as `ENOTSUPP` in the same place, which is no answer of yes.
fn kitty_said_ok(answer: &[u8]) -> bool {
    let mut rest = answer;
    while let Some(start) = position_of(rest, APC_OPENER) {
        let body = &rest[start + APC_OPENER.len()..];
        let Some(end) = position_of(body, STRING_TERMINATOR) else {
            return false;
        };
        let block = &body[..end];
        if block.first() == Some(&KITTY_GRAPHICS) {
            let separator = block.iter().position(|byte| *byte == KITTY_SEPARATOR);
            if let Some(separator) = separator {
                if block[separator + 1..].starts_with(KITTY_OK) {
                    return true;
                }
            }
        }
        rest = &body[end + STRING_TERMINATOR.len()..];
    }
    false
}

/// Whether an answer of the primary device attributes names sixel.
///
/// The answer is `ESC [ ? <parameters> c`, and the parameters are numbers
/// divided by a semicolon. Parameter 4 names sixel. The test is over the whole
/// parameter and not over the digit, because 14 and 41 hold a four and name
/// something else.
fn attributes_name_sixel(answer: &[u8]) -> bool {
    attributes_parameters(answer).is_some_and(|parameters| {
        parameters
            .split(|byte| *byte == KITTY_SEPARATOR)
            .any(|parameter| parameter == SIXEL_PARAMETER)
    })
}

/// The parameters of the first whole answer of the primary device attributes.
///
/// [`drain`] reads this to learn that the answer arrived, and
/// [`attributes_name_sixel`] reads it to learn what the answer says. One
/// reader of the shape keeps the two from disagreeing about where an answer
/// ends.
fn attributes_parameters(answer: &[u8]) -> Option<&[u8]> {
    let mut rest = answer;
    while let Some(start) = position_of(rest, ATTRIBUTES_OPENER) {
        let body = &rest[start + ATTRIBUTES_OPENER.len()..];
        // A control sequence ends at its final byte, which stands above every
        // parameter byte. An answer with no final byte is an answer cut short.
        let end = body.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
        if body[end] == ATTRIBUTES_FINAL {
            return Some(&body[..end]);
        }
        rest = &body[end + 1..];
    }
    None
}

/// Where `needle` starts in `haystack`.
fn position_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Ask the controlling terminal which image protocol it draws.
///
/// Gives [`None`] when there is no controlling terminal, when this run stands
/// in a background process group and therefore owns no terminal to ask (see
/// [`owns_the_terminal`]), when the terminal answers nothing inside `budget`,
/// or when the answer names neither protocol.
pub(crate) fn ask_the_terminal(budget: Duration) -> Option<AnsweredProtocol> {
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open(CONTROLLING_TERMINAL)
        .ok()?;
    let fd = terminal.as_raw_fd();
    let _raw = RawMode::of(fd)?;
    (&terminal).write_all(IMAGE_QUERY).ok()?;
    (&terminal).flush().ok()?;
    read_answer(&drain(fd, budget))
}

/// The terminal a program asks, whatever its standard output was pointed at.
const CONTROLLING_TERMINAL: &str = "/dev/tty";

/// The largest answer this module keeps.
///
/// Every answer of [`IMAGE_QUERY`] is far below this. The cap is here so that
/// a terminal which writes without stopping cannot grow the buffer.
const ANSWER_LIMIT: usize = 1024;

/// How much of an answer one read takes.
///
/// One byte, because the loop tests for a whole answer of the attributes after
/// each read. A larger read takes the bytes behind the answer as well, and
/// those bytes belong to whoever reads the descriptor next, so one byte is what
/// makes the rule of [`drain`] true. A whole answer is about twenty bytes, so
/// one probe pays about twenty pairs of select(2) and read(2) for it.
const READ_CHUNK: usize = 1;

/// Read the descriptor until the answer of the attributes arrives, until the
/// budget is spent, or until the cap is reached.
///
/// **The read stops at the answer of the attributes and takes no byte after
/// it.** Every byte after that one belongs to whoever types next, and a probe
/// that swallowed it would eat a keystroke of the user.
fn drain(fd: RawFd, budget: Duration) -> Vec<u8> {
    let deadline = Instant::now() + budget;
    let mut answer = Vec::new();
    while answer.len() < ANSWER_LIMIT {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        if !waits_for_a_byte(fd, left) {
            break;
        }
        let mut chunk = [0_u8; READ_CHUNK];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        let Ok(taken) = usize::try_from(taken) else {
            break;
        };
        if taken == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..taken]);
        if attributes_parameters(&answer).is_some() {
            break;
        }
    }
    answer
}

/// Wait up to `left` for one byte to arrive on `fd`.
///
/// # Why this is select(2) and not poll(2)
///
/// **poll(2) on macOS answers POLLNVAL for a terminal.** Measured 2026-09-06
/// against a tmux pane: the descriptor is a good one, the terminal has an
/// answer waiting, and poll(2) gives 1 with `revents` of `POLLNVAL`. A caller
/// that reads that as "no answer" gets silence from every terminal on the
/// platform, and silence from a terminal is indistinguishable from a terminal
/// that draws no image. select(2) answers the same question correctly there.
fn waits_for_a_byte(fd: RawFd, left: Duration) -> bool {
    // select(2) reads the descriptor as a bit in a fixed-width set, so a
    // descriptor above the width of that set cannot be asked about at all.
    let width = RawFd::try_from(libc::FD_SETSIZE).unwrap_or(RawFd::MAX);
    if fd < 0 || fd >= width {
        return false;
    }
    // SAFETY: `fd_set` is a plain bit array, and FD_ZERO fills the whole of it
    // before FD_SET writes one bit of it.
    let mut watched: libc::fd_set = unsafe { std::mem::zeroed() };
    // SAFETY: `watched` is one initialized set and `fd` is inside its width.
    unsafe {
        libc::FD_ZERO(&mut watched);
        libc::FD_SET(fd, &mut watched);
    }
    let mut budget = libc::timeval {
        tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_usec: libc::suseconds_t::try_from(left.subsec_micros()).unwrap_or(0),
    };
    // SAFETY: the set and the budget are owned here, and the two pointers that
    // name no set are null, which select(2) reads as "ask nothing of them".
    let ready = unsafe {
        libc::select(
            fd + 1,
            &mut watched,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut budget,
        )
    };
    ready > 0
}

/// Whether the process group of this run owns `fd`.
///
/// **A caller that does not own the terminal must write no terminal setting to
/// it.** tcsetattr(3) of such a caller sends SIGTTOU to the whole process group
/// of the caller, and the default action of SIGTTOU stops the process. A shell
/// that starts a script with `&` puts that script in a background process
/// group, so a probe with no guard here freezes the script on the one call that
/// asks for raw mode, and it prints nothing that says why.
///
/// A run that owns no terminal keeps the behavior it had before the probe
/// existed: it asks nothing, and the name the environment carries is the whole
/// of the answer.
///
/// A terminal that answers nothing to tcgetpgrp(3) is one this run does not own
/// either, because the answer of that call is the one thing that says it does.
fn owns_the_terminal(fd: RawFd) -> bool {
    // SAFETY: `fd` is the descriptor of a file this module holds open.
    let foreground = unsafe { libc::tcgetpgrp(fd) };
    // SAFETY: getpgrp takes no argument, reads no memory and fails for nothing.
    foreground != -1 && foreground == unsafe { libc::getpgrp() }
}

/// The terminal held in raw mode, and put back the way it was on the way out.
///
/// A canonical terminal gives a read nothing until a newline arrives, and no
/// answer of a terminal carries one. It also echoes, which would paint the
/// answer onto the screen of the user.
struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    /// Put `fd` in raw mode, or give [`None`] for a descriptor this process
    /// group does not own and for a descriptor that refuses.
    fn of(fd: RawFd) -> Option<Self> {
        if !owns_the_terminal(fd) {
            return None;
        }
        // SAFETY: `termios` is a plain C struct, and tcgetattr fills the whole
        // of it before anything reads it.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is the descriptor of a file this module holds open.
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        // SAFETY: `raw` is a copy of a structure tcgetattr filled.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: same descriptor, and `raw` is fully initialized.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open, because this value lives no
        // longer than the file it was made from.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_that_names_sixel_gives_sixel() {
        // tmux 3.7c answers this, measured 2026-09-06. Parameter 4 names sixel.
        assert_eq!(
            read_answer(b"\x1b[?1;2;4c"),
            Some(AnsweredProtocol::Sixel),
            "parameter 4 of the primary device attributes names sixel"
        );
    }

    #[test]
    fn a_zellij_answer_names_sixel() {
        // zellij 0.45.0 answers this, measured 2026-09-06.
        assert_eq!(
            read_answer(b"\x1bP>|Zellij(4500)\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Sixel)
        );
    }

    #[test]
    fn an_ok_of_the_kitty_query_gives_kitty() {
        assert_eq!(
            read_answer(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Kitty),
            "a terminal that answers both draws the kitty transmission"
        );
    }

    #[test]
    fn an_answer_that_names_neither_gives_nothing() {
        assert_eq!(read_answer(b"\x1b[?62;22c"), None);
    }

    #[test]
    fn a_refusal_of_the_kitty_query_is_no_kitty_answer() {
        // The specification answers a refused query with a code, not with OK.
        assert_eq!(
            read_answer(b"\x1b_Gi=31;ENOTSUPP:nope\x1b\\\x1b[?62;22c"),
            None
        );
    }

    #[test]
    fn a_parameter_that_merely_holds_a_four_is_no_sixel() {
        // 14 and 41 are not 4. A test of the digit alone would take them.
        assert_eq!(read_answer(b"\x1b[?14;41c"), None);
    }

    #[test]
    fn silence_gives_nothing() {
        assert_eq!(read_answer(b""), None);
    }

    #[test]
    fn a_descriptor_outside_the_set_of_select_waits_for_nothing() {
        // select(2) reads the descriptor as a bit of a fixed-width set, so a
        // descriptor above that width would write past the end of it.
        let width = i32::try_from(libc::FD_SETSIZE).expect("the set of select is far below i32");
        assert!(!waits_for_a_byte(width, Duration::from_millis(1)));
        assert!(!waits_for_a_byte(-1, Duration::from_millis(1)));
    }

    #[test]
    fn the_read_stops_at_the_answer_and_leaves_what_follows() {
        // A user who types inside the budget writes those bytes to the same
        // descriptor the answer of the terminal arrives on. The probe owes
        // every one of them to whoever reads the descriptor next.
        let mut ends = [0_i32; 2];
        // SAFETY: pipe(2) fills the two descriptors of an array this test owns.
        let made = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(made, 0, "pipe(2) gives a pair of descriptors");
        let [reader, writer] = ends;

        let sent = b"\x1b[?1;2;4cxyz";
        // SAFETY: the buffer is owned here and the length is its own.
        let put = unsafe { libc::write(writer, sent.as_ptr().cast(), sent.len()) };
        assert_eq!(
            put,
            isize::try_from(sent.len()).expect("the answer is far below the size of a pipe"),
            "the whole of the answer and the keystrokes reach the pipe"
        );

        let answer = drain(reader, QUERY_BUDGET);
        assert_eq!(
            read_answer(&answer),
            Some(AnsweredProtocol::Sixel),
            "the read takes the whole answer of the attributes"
        );

        // A closed write end ends the second read at once, whatever it holds.
        // SAFETY: this test opened the descriptor and nothing else holds it.
        unsafe { libc::close(writer) };
        let mut left = [0_u8; 16];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(reader, left.as_mut_ptr().cast(), left.len()) };
        // SAFETY: this test opened the descriptor and nothing else holds it.
        unsafe { libc::close(reader) };
        let taken =
            usize::try_from(taken).expect("a read of a pipe with no writer fails for nothing");
        assert_eq!(
            &left[..taken],
            b"xyz",
            "the bytes after the answer stay for the reader that comes next"
        );
    }

    #[test]
    fn the_query_ends_with_the_attributes_request() {
        assert!(
            IMAGE_QUERY.ends_with(b"\x1b[c"),
            "the answer of the attributes request is what ends the read"
        );
    }

    /// The budget of the probe that the background grandchild runs.
    ///
    /// Nothing stands behind the pseudo-terminal to answer, so every run of
    /// this test spends the whole of this budget. It is short for that reason.
    const BACKGROUND_BUDGET: Duration = Duration::from_millis(50);

    /// How long this test waits for the child to report.
    const REPORT_BUDGET: Duration = Duration::from_secs(10);

    /// How long this test waits between two reads of the state of the child.
    const REPORT_POLL: Duration = Duration::from_millis(10);

    /// How many bytes the child writes to name the grandchild.
    const PID_BYTES: usize = std::mem::size_of::<libc::pid_t>();

    /// What the child exits with for a grandchild that stopped.
    const STOPPED: libc::c_int = 17;

    /// What the child exits with for a grandchild that came back.
    const CAME_BACK: libc::c_int = 19;

    /// What the child exits with when a call of its own failed.
    const FAILED: libc::c_int = 21;

    /// What the child exits with when the guard refused the owner itself.
    ///
    /// The verdict of the grandchild alone says nothing about a guard that
    /// answers no to every caller, and such a guard takes the probe out of
    /// service on every terminal. So the child, which owns the terminal, asks
    /// the guard about itself and reports what it said.
    const OWNER_REFUSED: libc::c_int = 23;

    /// Ask `terminal` from a process group that does not own it, and exit with
    /// the verdict.
    ///
    /// This runs in a child of a fork, and the process it forked from is a test
    /// binary that holds many threads. So the block below holds plain libc
    /// calls alone and leaves through `_exit`, which runs no destructor of the
    /// parent.
    ///
    /// The child takes `terminal` as its controlling terminal, and its process
    /// group is the foreground group of that terminal. The grandchild leaves
    /// that group, which is what puts it in the background, and asks the
    /// terminal from there. A grandchild that stops instead of coming back is
    /// the defect. The child names the grandchild on `report` first, so that a
    /// run which has to clean up after a timeout can reach it.
    ///
    /// The child also reports what the guard says about the child itself, which
    /// owns the terminal. Both directions of the guard are then in one verdict.
    fn ask_from_the_background(terminal: RawFd, report: RawFd) -> ! {
        // SAFETY: every call below reads no memory of this process, except the
        // bytes of `named` that the write names and the status word that
        // waitpid fills, and this function owns both of them. `terminal` and
        // `report` are descriptors the test opened and still holds. The fork
        // gave this process a new process id and left it in the process group
        // of the test, so it leads no process group and setsid cannot fail for
        // the one reason it has.
        unsafe {
            if libc::setsid() == -1 {
                libc::_exit(FAILED);
            }
            #[allow(
                clippy::disallowed_methods,
                reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of this child, and termsize offers no call for that"
            )]
            if libc::ioctl(terminal, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                libc::_exit(FAILED);
            }

            // setsid made this child the leader of a session and TIOCSCTTY made
            // its process group the foreground group of the terminal, so this
            // child owns the terminal and the guard must say so.
            if !owns_the_terminal(terminal) {
                libc::_exit(OWNER_REFUSED);
            }

            let grandchild = libc::fork();
            if grandchild == -1 {
                libc::_exit(FAILED);
            }
            if grandchild == 0 {
                // A process group of its own is a background group of this
                // terminal, because the foreground group is still the one of
                // the child. This is the shape of a script that a shell
                // started with `&`.
                if libc::setpgid(0, 0) == -1 {
                    libc::_exit(FAILED);
                }
                let _answer = ask_the_terminal(BACKGROUND_BUDGET);
                libc::_exit(0);
            }

            let named = grandchild.to_ne_bytes();
            let _sent = libc::write(report, named.as_ptr().cast(), named.len());

            let mut status: libc::c_int = 0;
            if libc::waitpid(grandchild, &mut status, libc::WUNTRACED) == -1 {
                libc::_exit(FAILED);
            }
            if libc::WIFSTOPPED(status) {
                libc::kill(grandchild, libc::SIGCONT);
                libc::kill(grandchild, libc::SIGKILL);
                libc::waitpid(grandchild, std::ptr::null_mut(), 0);
                libc::_exit(STOPPED);
            }
            libc::_exit(CAME_BACK)
        }
    }

    /// The process id the child wrote to `reader`, or [`None`] for a child that
    /// named nothing inside `budget`.
    fn read_the_pid(reader: RawFd, budget: Duration) -> Option<libc::pid_t> {
        if !waits_for_a_byte(reader, budget) {
            return None;
        }
        let mut named = [0_u8; PID_BYTES];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(reader, named.as_mut_ptr().cast(), named.len()) };
        (usize::try_from(taken) == Ok(named.len())).then(|| libc::pid_t::from_ne_bytes(named))
    }

    #[test]
    fn a_run_in_the_background_asks_the_terminal_nothing() {
        // tcsetattr(3) of a caller that stands in a background process group
        // sends SIGTTOU to the whole of that group, and the default action of
        // SIGTTOU stops the process. A script that a shell started with `&`
        // stands in such a group, so a probe that puts the terminal in raw
        // mode there freezes the script and prints nothing that says why. This
        // test builds that process group and asks whether the probe comes back
        // out of it.
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // SAFETY: openpty writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The three null
        // pointers ask for the default terminal modes, for no name of the
        // slave device and for the default size of the window. The probe reads
        // no window, so no size of one reaches what this test asserts.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            opened,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        let mut ends = [0_i32; 2];
        // SAFETY: pipe(2) fills the two descriptors of an array this test owns.
        let made = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(made, 0, "pipe(2) gives a pair of descriptors");
        let [reader, writer] = ends;

        // SAFETY: fork(2) reads nothing of this process. The child leaves
        // through `_exit` alone, so it runs no destructor of this one.
        let child = unsafe { libc::fork() };
        assert!(child != -1, "fork(2) must give a child");
        if child == 0 {
            ask_from_the_background(slave, writer);
        }

        // SAFETY: this test opened the descriptor and the child holds a copy of
        // its own. A closed copy here ends the read below for a child that
        // names nothing.
        unsafe { libc::close(writer) };
        let grandchild = read_the_pid(reader, REPORT_BUDGET);

        let deadline = Instant::now() + REPORT_BUDGET;
        let mut status: libc::c_int = 0;
        let mut reported = 0;
        while Instant::now() < deadline {
            // SAFETY: `status` is owned here, and `child` is a child of this
            // process, so waitpid reads no other family.
            reported = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
            if reported != 0 {
                break;
            }
            std::thread::sleep(REPORT_POLL);
        }

        if reported != child {
            // Take the family down, so that a run which timed out leaves no
            // stopped process behind. The grandchild goes first: it stands in
            // a process group of its own, and a group with no living parent
            // outside it takes no job control signal at all.
            // SAFETY: kill and waitpid read no memory of this process, and both
            // process ids below name a member of this family.
            unsafe {
                if let Some(grandchild) = grandchild {
                    libc::kill(grandchild, libc::SIGCONT);
                    libc::kill(grandchild, libc::SIGKILL);
                }
                libc::kill(child, libc::SIGKILL);
                libc::waitpid(child, std::ptr::null_mut(), 0);
            }
        }

        // Every descriptor goes back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened all three descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(reader);
            libc::close(slave);
            libc::close(master);
        }

        assert_eq!(
            reported, child,
            "the child must report inside {REPORT_BUDGET:?}"
        );
        assert!(
            libc::WIFEXITED(status),
            "the child must exit, and it exited for a signal instead"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            CAME_BACK,
            "a probe that stands in a background process group must come back. \
             {STOPPED} says the grandchild stopped, which is the SIGTTOU that \
             tcsetattr(3) sends to a background caller. {OWNER_REFUSED} says \
             the guard refused the child, which owns the terminal and must be \
             free to ask it"
        );
    }
}
