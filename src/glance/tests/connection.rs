#![deny(unsafe_code)]
#![cfg(target_os = "macos")]
//! Tests of the connection to a second Mac. A fake script stands in for `ssh`
//! and runs the built `glance --stream`, so no test changes `PATH`.

use glance::connection::{Connection, Destination, Event, RemoteBinary};
use glance::host::HostName;
use glance::interval::Interval;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The time that a test waits for one event.
const EVENT_WAIT: Duration = Duration::from_secs(60);

/// The interval of the stream.
const INTERVAL: &str = "0.25";

/// The destination that the fake `ssh` gets.
const DESTINATION: &str = "m1-ts";

/// The mode of a script.
const EXECUTABLE: u32 = 0o755;

/// The number of samples that a finite stream gives.
const THREE: usize = 3;

/// The real binary.
const GLANCE: &str = env!("CARGO_BIN_EXE_glance");

/// The count of the directories that this process made.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Starts the binary once, so the first test does not measure its first load.
static WARM: Once = Once::new();

/// A temporary directory that goes away with the value.
struct Scratch(PathBuf);

impl Scratch {
    /// Makes a directory that no other test and no other run shares.
    fn new() -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "glance-connection-{}-{nanos}-{number}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("the directory is made");
        Scratch(path)
    }

    /// A path inside the directory.
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    /// Writes an executable `#!/bin/sh` script and returns its path.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.path(name);
        write_script(&path, body);
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes an executable `#!/bin/sh` script at `path`, and makes its directory.
fn write_script(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("the directory is made");
    }
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("the script is written");
    fs::set_permissions(path, fs::Permissions::from_mode(EXECUTABLE))
        .expect("the script is executable");
}

/// The path as quoted text for a script.
fn quoted(path: &Path) -> String {
    shellquote::shell_quote(path.to_str().expect("the path is UTF-8"))
}

/// The real `glance` as quoted text for a script.
fn glance() -> String {
    quoted(Path::new(GLANCE))
}

/// The interval of the tests.
fn interval() -> Interval {
    INTERVAL.parse().expect("an interval")
}

/// Starts a connection through `program` after the binary is warm.
fn start(program: &Path, remote_binary: &RemoteBinary) -> Connection {
    WARM.call_once(|| {
        let status = childenv::fixture_command(GLANCE)
            .arg("--version")
            .output()
            .expect("the binary starts")
            .status;
        assert!(status.success(), "{status}");
    });
    let destination: Destination = DESTINATION.parse().expect("a destination");
    Connection::start(program, &destination, interval(), remote_binary).expect("the child starts")
}

/// Reads events until `Ended`, and panics with the events so far on a timeout.
fn events_until_end(connection: &Connection) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match connection.receiver().recv_timeout(EVENT_WAIT) {
            Ok(event) => {
                let ended = matches!(event, Event::Ended { .. });
                events.push(event);
                if ended {
                    return events;
                }
            }
            Err(error) => panic!("no end after {error}: {events:?}"),
        }
    }
}

/// The number of `Sample` events.
fn sample_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::Sample { .. }))
        .count()
}

/// Checks `THREE` samples and then one end.
fn assert_three_samples_then_end(events: &[Event]) {
    assert_eq!(events.len(), THREE + 1, "{events:?}");
    assert_eq!(sample_count(events), THREE, "{events:?}");
    assert!(
        matches!(events.last(), Some(Event::Ended { .. })),
        "{events:?}"
    );
}

/// The fake that runs the command of `ssh` with `HOME` set to `home`.
fn home_fake(scratch: &Scratch, home: &Path) -> PathBuf {
    scratch.script(
        "ssh-home",
        &format!(
            "for last; do :; done\nHOME={} exec /bin/sh -c \"$last\"",
            quoted(home)
        ),
    )
}

#[test]
fn the_fake_gets_the_exact_arguments_and_the_samples_arrive() {
    let scratch = Scratch::new();
    let args = scratch.path("args.txt");
    let fake = scratch.script(
        "ssh",
        &format!(
            "printf '%s\\n' \"$@\" > {}\nexec {} --stream --count 3 --interval 0.25",
            quoted(&args),
            glance()
        ),
    );
    let connection = start(&fake, &RemoteBinary::default());
    assert_three_samples_then_end(&events_until_end(&connection));
    let text = fs::read_to_string(&args).expect("the arguments file");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        [
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            DESTINATION,
            "~/'.cargo/bin/glance' --stream --interval 0.25",
        ]
    );
}

#[test]
fn the_remote_shell_expands_the_tilde_of_the_default() {
    let scratch = Scratch::new();
    write_script(
        &scratch.path(".cargo/bin/glance"),
        &format!("exec {} \"$@\" --count 3", glance()),
    );
    let fake = home_fake(&scratch, &scratch.0);
    let connection = start(&fake, &RemoteBinary::default());
    assert_three_samples_then_end(&events_until_end(&connection));
}

#[test]
fn a_space_and_a_quote_in_the_remote_binary_stay_in_one_word() {
    let scratch = Scratch::new();
    let wrapper = format!("exec {} \"$@\" --count 3", glance());
    write_script(&scratch.path("it's a bin/glance"), &wrapper);
    write_script(&scratch.path("a b/glance"), &wrapper);
    let fake = home_fake(&scratch, &scratch.0);
    let tilde: RemoteBinary = "~/it's a bin/glance".parse().expect("a binary");
    let connection = start(&fake, &tilde);
    assert_three_samples_then_end(&events_until_end(&connection));
    let absolute: RemoteBinary = scratch
        .path("a b/glance")
        .to_str()
        .expect("UTF-8")
        .parse()
        .expect("a binary");
    let connection = start(&fake, &absolute);
    assert_three_samples_then_end(&events_until_end(&connection));
}

#[test]
fn the_last_line_of_standard_error_is_the_reason() {
    let scratch = Scratch::new();
    let fake = scratch.script(
        "ssh",
        "printf '%s\\n' 'zsh:1: command not found: glance' >&2\nexit 127",
    );
    let connection = start(&fake, &RemoteBinary::default());
    assert_eq!(
        events_until_end(&connection),
        [Event::Ended {
            reason: "zsh:1: command not found: glance".to_owned()
        }]
    );
}

#[test]
fn a_silent_child_ends_with_its_exit_status() {
    let scratch = Scratch::new();
    let fake = scratch.script("ssh", "exit 3");
    let connection = start(&fake, &RemoteBinary::default());
    assert_eq!(
        events_until_end(&connection),
        [Event::Ended {
            reason: "the connection ended with exit status: 3".to_owned()
        }]
    );
}

#[test]
fn a_line_that_is_not_json_is_a_read_error_and_the_stream_goes_on() {
    let scratch = Scratch::new();
    let fake = scratch.script(
        "ssh",
        &format!(
            "echo 'not json'\nexec {} --stream --count 1 --interval 0.25",
            glance()
        ),
    );
    let connection = start(&fake, &RemoteBinary::default());
    let events = events_until_end(&connection);
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(
        matches!(&events[0], Event::ReadError { host: None, .. }),
        "{events:?}"
    );
    assert!(matches!(&events[1], Event::Sample { .. }), "{events:?}");
    assert!(matches!(&events[2], Event::Ended { .. }), "{events:?}");
}

#[test]
fn an_error_line_of_the_remote_mac_names_its_host() {
    let scratch = Scratch::new();
    let fake = scratch.script(
        "ssh",
        "printf '%s\\n' '{\"host\":\"m1\",\"error\":\"cannot read x: y\"}'\nexit 0",
    );
    let connection = start(&fake, &RemoteBinary::default());
    let events = events_until_end(&connection);
    assert_eq!(
        events[0],
        Event::ReadError {
            host: Some(HostName::from_raw(b"m1")),
            reason: "cannot read x: y".to_owned()
        }
    );
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(&events[1], Event::Ended { .. }), "{events:?}");
}

#[test]
fn a_stop_kills_and_reaps_the_child() {
    let scratch = Scratch::new();
    let pid_file = scratch.path("pid.txt");
    let fake = scratch.script(
        "ssh",
        &format!(
            "echo $$ > {}\nexec {} --stream --count 240 --interval 0.25",
            quoted(&pid_file),
            glance()
        ),
    );
    let connection = start(&fake, &RemoteBinary::default());
    let first = connection
        .receiver()
        .recv_timeout(EVENT_WAIT)
        .expect("a first event");
    assert!(matches!(first, Event::Sample { .. }), "{first:?}");
    let pid = fs::read_to_string(&pid_file).expect("the pid file");
    connection.stop();
    let output = childenv::fixture_command("ps")
        .args(["-p", pid.trim(), "-o", "pid="])
        .output()
        .expect("ps starts");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "",
        "the child {pid} still exists"
    );
}
