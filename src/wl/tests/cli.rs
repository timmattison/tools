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
