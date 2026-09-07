//! Black-box tests for the `wl` binary, driving the real CLI end to end.
//!
//! These tests use only ports that nothing can be listening on, so concurrent
//! test runs stay isolated (see the parallel-safety note in the project
//! guidelines).

use std::io;
use std::net::TcpListener;
use std::process::Command;

/// Port 1 (`tcpmux`) is below every platform's privileged threshold and holds
/// no service on a developer machine. An unprivileged process is therefore
/// *refused* the loopback bind that `wl` uses to probe the port, while nothing
/// is listening there — exactly the case where `wl` must not claim the port is
/// free.
const REFUSED_UNUSED_PORT: u16 = 1;

/// A bind the kernel refuses proves nothing about the port, so `wl` must not
/// report it as free.
#[test]
fn refused_probe_is_not_reported_as_free() {
    // Decide whether this test applies: only a process the kernel *refuses*
    // can observe the ambiguity.
    match TcpListener::bind(("127.0.0.1", REFUSED_UNUSED_PORT)) {
        // Privileged enough to bind it: the probe has nothing to be uncertain
        // about, so there is nothing here to assert.
        Ok(_) => return,
        // Some other failure (the port really is held, the address is missing):
        // not the case under test.
        Err(e) if e.kind() != io::ErrorKind::PermissionDenied => return,
        Err(_) => {}
    }

    let output = Command::new(env!("CARGO_BIN_EXE_wl"))
        .arg(REFUSED_UNUSED_PORT.to_string())
        .output()
        .expect("should be able to run the freshly built wl binary");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("No processes listening"),
        "the kernel refused wl's own bind probe on port {REFUSED_UNUSED_PORT}, \
         so wl cannot know whether anything is listening there, yet it reported \
         the port as free. stdout was: {stdout}"
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

    let output = Command::new(env!("CARGO_BIN_EXE_wl"))
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

/// The other direction of the same rule: an answer that named nobody is the
/// answer a hidden process explains, so the note belongs there.
#[test]
fn privilege_note_when_no_process_is_named() {
    // Decide whether this test applies. A process that can bind port 1 is
    // privileged, and a privileged run earns no note at all, so there is
    // nothing here to assert.
    match TcpListener::bind(("127.0.0.1", REFUSED_UNUSED_PORT)) {
        Ok(_) => return,
        // Some other failure (the port really is held, the address is missing):
        // not the case under test.
        Err(e) if e.kind() != io::ErrorKind::PermissionDenied => return,
        Err(_) => {}
    }

    let output = Command::new(env!("CARGO_BIN_EXE_wl"))
        .arg(REFUSED_UNUSED_PORT.to_string())
        .output()
        .expect("should be able to run the freshly built wl binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stdout.contains("PID:"),
        "nothing listens on port {REFUSED_UNUSED_PORT}, so wl should have named \
         no process. stdout was: {stdout}"
    );
    assert!(
        stderr.contains("note: running without root"),
        "wl named no process on port {REFUSED_UNUSED_PORT} while running \
         unprivileged, and a process this user cannot see explains that answer, \
         so the note belongs on stderr. stderr was: {stderr}"
    );
}
