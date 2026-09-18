//! A renderer that panics ends the process with `SIGABRT`.
//!
//! The render callback runs on the audio thread of Core Audio. A panic that
//! unwinds out of it runs into frames that Core Audio compiled, and nobody
//! knows what they do with it. So the callback catches the panic, says so on
//! stderr, and aborts the process.
//!
//! The process that panics is a child: this test binary itself, started again
//! with the name of an ignored helper test and an environment variable that
//! tells the helper to run.

// Core Audio exists on macOS only.
#![cfg(target_os = "macos")]

use std::env;
use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use popstop::output::{OutputUnit, Render};
use popstop::signal::SampleRate;

/// The environment variable that tells the helper to run. The helper does
/// nothing when the variable is absent.
const HELPER_VARIABLE: &str = "POPSTOP_RENDER_PANIC_HELPER";

/// The name of the helper test.
const HELPER_TEST_NAME: &str = "render_panic_helper_plays_a_renderer_that_panics";

/// The message of the panic in the renderer of the helper.
const PANIC_MESSAGE: &str = "the renderer of the helper panics on purpose";

/// The text that the Rust runtime writes when foreign code catches a Rust
/// panic and drops it. Core Audio does that when a panic unwinds into it.
const CAUGHT_BY_FOREIGN_CODE: &str = "Rust panics must be rethrown";

/// The text that tells the user why the process stopped.
const ABORT_MESSAGE: &str = "a renderer panicked on the audio thread";

/// The longest time the helper lives. The abort ends it long before, and the
/// bound makes sure that a helper that survives the panic ends.
const HELPER_LIFETIME: Duration = Duration::from_secs(5);

/// The longest time the test waits for the helper to end.
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

/// The time between two looks at the helper.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The number of `SIGABRT`. POSIX sets it to 6.
const SIGABRT: i32 = 6;

/// The sample rate of the stream, in hertz.
const RATE_HZ: f64 = 48_000.0;

/// A renderer that panics at its first call.
struct Panicking;

impl Render for Panicking {
    fn render(&mut self, _interleaved: &mut [f32], _channels: usize) {
        panic!("{PANIC_MESSAGE}");
    }
}

/// The helper process. A drop kills and reaps it, so a test that fails early
/// leaves no process.
struct Helper(Child);

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for the helper to end, for `timeout` at most. Gives its exit status
/// and its stderr.
fn wait_for_end(helper: &mut Helper, timeout: Duration) -> (ExitStatus, String) {
    let mut stderr_pipe = helper.0.stderr.take().expect("the stderr of the helper");
    let reader = thread::spawn(move || {
        let mut stderr = String::new();
        let _ = stderr_pipe.read_to_string(&mut stderr);
        stderr
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = helper.0.try_wait().expect("look at the helper") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the helper did not end within {timeout:?}"
        );
        thread::sleep(POLL_INTERVAL);
    };
    let stderr = reader.join().expect("read the stderr of the helper");
    (status, stderr)
}

#[test]
fn a_renderer_that_panics_aborts_the_process_and_says_why() {
    let mut helper = Helper(
        Command::new(env::current_exe().expect("the path of this test binary"))
            .args(["--exact", HELPER_TEST_NAME, "--ignored", "--nocapture"])
            .env(HELPER_VARIABLE, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the helper"),
    );

    let (status, stderr) = wait_for_end(&mut helper, HELPER_TIMEOUT);

    assert_eq!(
        status.signal(),
        Some(SIGABRT),
        "SIGABRT did not end the helper: {status}. Its stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(PANIC_MESSAGE),
        "the stderr of the helper does not show the panic. Its stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(CAUGHT_BY_FOREIGN_CODE),
        "the panic unwound into Core Audio, which caught it. Its stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(ABORT_MESSAGE),
        "the stderr of the helper does not say why the process stopped. Its stderr:\n{stderr}"
    );
}

/// The helper that the panic test starts in a child process.
///
/// It starts an output unit whose renderer panics, and waits for the abort.
#[test]
#[ignore = "the panic test starts this helper in a child process"]
fn render_panic_helper_plays_a_renderer_that_panics() {
    if env::var_os(HELPER_VARIABLE).is_none() {
        return;
    }
    let rate = SampleRate::new(RATE_HZ).expect("48000 Hz is a valid sample rate");
    let unit = OutputUnit::start(rate, Panicking).unwrap_or_else(|error| {
        panic!(
            "the output unit did not start: {error}. This test plays on the default output \
             device, so the machine must have one"
        )
    });
    thread::sleep(HELPER_LIFETIME);
    drop(unit);
}
