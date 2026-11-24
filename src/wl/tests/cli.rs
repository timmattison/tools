//! Black-box tests for the `wl` binary, driving the real CLI end to end.
//!
//! These tests use only ports that nothing can be listening on, so concurrent
//! test runs stay isolated (see the parallel-safety note in the project
//! guidelines).

use std::net::TcpListener;

/// Port 1 (`tcpmux`) is below every platform's privileged threshold and holds
/// no service on a developer machine. An unprivileged process is therefore
/// *refused* the loopback bind, while nothing is listening there — the case a
/// bind probe could never answer, and the case the kernel's own list of
/// listening sockets answers without any privilege at all.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const REFUSED_UNUSED_PORT: u16 = 1;

/// A reader of the kernel's socket table needs no privileges, so `wl` can
/// answer for a privileged port that holds no service.
///
/// This is the behaviour the socket-table reader adds. A bind probe could not
/// reach the question on a port below the privileged threshold — the kernel
/// refuses the bind first — so the answer there was always "cannot tell". The
/// kernel's own list of listening sockets is readable by any user, so a run
/// that names no process on port 1 can now say so.
///
/// Gated to the platforms that have a reader. Every other platform answers
/// "cannot tell" for every port, which is what the module documentation of
/// `socket_table` records as the remaining gap.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn unheld_privileged_port_is_reported_free() {
    let output = childenv::fixture_command(env!("CARGO_BIN_EXE_wl"))
        .arg(REFUSED_UNUSED_PORT.to_string())
        .output()
        .expect("should be able to run the freshly built wl binary");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("No processes listening"),
        "nothing is listening on port {REFUSED_UNUSED_PORT}, and reading the \
         kernel's socket table needs no privileges, so wl should have reported \
         the port free instead of declining to answer. stdout was: {stdout}"
    );
}

/// The privilege note explains an answer that names nobody. When `wl` names the
/// process the user asked about, the answer is complete and the advice is noise.
///
/// No applicability gate is needed here: under root the note never prints at
/// all, so the "stderr carries no note" half is simply true there, and the
/// listener this test holds is owned by this test's own user, so `wl` can see it
/// whatever privileges the run has.
#[test]
fn no_privilege_note_when_a_process_is_named() {
    // The kernel picks the port, so two copies of this test that run at the
    // same time never ask about the same one.
    let listener =
        TcpListener::bind("127.0.0.1:0").expect("should be able to bind an ephemeral port");
    let port = listener
        .local_addr()
        .expect("bound listener must have a local address")
        .port();

    let output = childenv::fixture_command(env!("CARGO_BIN_EXE_wl"))
        .arg(port.to_string())
        .output()
        .expect("should be able to run the freshly built wl binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("PID:"),
        "this test holds port {port}, so wl should have named the owning process. \
         stdout was: {stdout}"
    );
    assert!(
        !stderr.contains("note: running without root"),
        "wl named the process listening on port {port}, so the answer is complete \
         and the privilege note is noise. stderr was: {stderr}"
    );

    drop(listener);
}

/// Free is the answer of a complete read of the kernel's socket table, which
/// lists the sockets of every user. After Free no hidden process can exist, so
/// the privilege note would be false, and `sudo` gives the same answer.
///
/// No applicability gate is needed: under root the note never prints, so "no
/// note" is true there too. Gated to the platforms that have a reader that can
/// answer Free.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn no_privilege_note_when_the_port_is_free() {
    let output = childenv::fixture_command(env!("CARGO_BIN_EXE_wl"))
        .arg(REFUSED_UNUSED_PORT.to_string())
        .output()
        .expect("should be able to run the freshly built wl binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stdout.contains("No processes listening"),
        "this test needs the Free answer for port {REFUSED_UNUSED_PORT}. \
         stdout was: {stdout}"
    );
    assert!(
        !stderr.contains("note: running without root"),
        "wl read the whole kernel socket table and found port {REFUSED_UNUSED_PORT} \
         free, so no hidden process exists and the privilege note is false. \
         stderr was: {stderr}"
    );
}
