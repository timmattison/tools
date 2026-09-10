//! The child of a pseudo-terminal holds that terminal on its standard streams
//! and on no other descriptor.
//!
//! `openpty` opens both ends without the close-on-exec flag. Without the flag,
//! each end reaches every child that the test process starts, and each child
//! then holds the terminal open for as long as it runs. The read of the master
//! end ends only when the last copy of the slave end closes, so a copy in a
//! child that outlives the run holds that read open. A pseudo-terminal is also
//! a slot of a supply that every process of the machine shares.
//!
//! The test stands in a file of its own, so its target holds one test. A second
//! test in the same process can open a pseudo-terminal in the few instructions
//! between `openpty` and the call that sets the flag. A child that starts in
//! that window takes a copy, and this test then fails for a reason that is not
//! its own.

#![cfg(unix)]

use std::process::Command;

use gitscratch::testing::pty::Pty;

/// The width of the terminal here. The test reads no width, and any width
/// above zero serves.
const COLUMNS: u16 = 80;

/// A script that prints each descriptor of the shell above its three standard
/// streams that is a terminal, as numbers followed by a space.
///
/// `ls` lists the descriptors of its own process. That process holds every
/// descriptor that the shell holds, and the shell then asks of each number
/// whether it names a terminal in the shell itself. The one descriptor that
/// `ls` holds alone, the directory it reads, names nothing in the shell.
const TERMINALS_ABOVE_THE_STANDARD_STREAMS: &str = r#"for fd in $(ls /dev/fd); do if [ "$fd" -gt 2 ] && [ -t "$fd" ]; then printf '%s ' "$fd"; fi; done"#;

/// A `sh -c` command that runs [`TERMINALS_ABOVE_THE_STANDARD_STREAMS`].
fn listing() -> Command {
    let mut command = Command::new("sh");
    command.arg("-c").arg(TERMINALS_ABOVE_THE_STANDARD_STREAMS);
    command
}

/// The terminal reaches the child on its standard streams alone.
///
/// The control runs the same listing in a plain child first, while this
/// process holds no pseudo-terminal. A terminal that the environment of the run
/// gives every child then shows in both answers. So the test holds on a
/// machine whose shell leaks such a terminal, and only a copy that the helper
/// leaks makes the two answers differ.
#[test]
fn the_terminal_reaches_the_child_on_its_standard_streams_alone() {
    let control = listing().output().expect("the control child must start");
    assert!(
        control.status.success(),
        "the listing must run in a plain child: {}",
        String::from_utf8_lossy(&control.stderr)
    );

    let output = Pty::open(COLUMNS).run_with_stdout_on_terminal(listing());

    assert!(
        output.status.success(),
        "the listing must run on the terminal: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&control.stdout),
        "a child of the terminal must hold no terminal above its standard \
         streams that a plain child does not hold. Each extra number is a copy \
         of an end of the terminal that reached the child"
    );
}
