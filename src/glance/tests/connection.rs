#![deny(unsafe_code)]
#![cfg(target_os = "macos")]
//! Tests of the connection to a second Mac. A fake script stands in for `ssh`
//! and runs the built `glance --stream`, so no test changes `PATH`.

use gitscratch::testing::executable::write_executable;
use glance::connection::{
    Connection, Destination, Event, Feed, HostNaming, Remote, RemoteBinary, FIRST_RETRY_DELAY,
};
use glance::host::HostName;
use glance::interval::Interval;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The time that a test waits for one event.
const EVENT_WAIT: Duration = Duration::from_secs(60);

/// The interval of the stream.
const INTERVAL: &str = "0.25";

/// The destination that the fake `ssh` gets.
const DESTINATION: &str = "m1-ts";

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
    write_executable(path, format!("#!/bin/sh\n{body}\n")).expect("the script is written");
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

/// Starts the binary once in this process, so no test measures its first load.
fn warm() {
    WARM.call_once(|| {
        let status = childenv::fixture_command(GLANCE)
            .arg("--version")
            .output()
            .expect("the binary starts")
            .status;
        assert!(status.success(), "{status}");
    });
}

/// Starts a connection through `program` after the binary is warm.
fn start(program: &Path, remote_binary: &RemoteBinary) -> Connection {
    warm();
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

/// The pause between two polls of a feed while a test waits for an event.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// One step of time just before a connection is due.
const JUST_BEFORE: Duration = Duration::from_millis(1);

/// The reason of the end of the fake that exits with status 3.
const EXIT_3: &str = "the connection ended with exit status: 3";

/// The second Mac of the feed tests, with the default remote binary.
fn remote() -> Remote {
    Remote {
        destination: DESTINATION.parse().expect("a destination"),
        remote_binary: RemoteBinary::default(),
        naming: HostNaming::Destination,
    }
}

/// Polls `feed` at the time `now` until an `Ended` event arrives.
///
/// Gives all of the events, and panics with them after `EVENT_WAIT`.
fn poll_until_end(feed: &mut Feed, now: Instant) -> Vec<Event> {
    let deadline = Instant::now() + EVENT_WAIT;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        let batch = feed.poll(now);
        let ended = batch
            .iter()
            .any(|event| matches!(event, Event::Ended { .. }));
        events.extend(batch);
        if ended {
            return events;
        }
        std::thread::sleep(POLL_PAUSE);
    }
    panic!("no end within {EVENT_WAIT:?}: {events:?}");
}

/// The number of starts that a fake wrote into `starts`.
fn start_count(starts: &Path) -> usize {
    fs::read_to_string(starts).map_or(0, |text| text.lines().count())
}

/// The script line that adds one start to the file `starts`.
fn count_start(starts: &Path) -> String {
    format!("printf 'x\\n' >> {}", quoted(starts))
}

#[test]
fn a_feed_waits_the_delay_of_the_backoff_after_each_end() {
    let scratch = Scratch::new();
    let starts = scratch.path("starts");
    let fake = scratch.script("ssh", &format!("{}\nexit 3", count_start(&starts)));
    warm();
    let mut feed = Feed::new(&fake, remote(), interval());
    let first_end = Instant::now();

    let events = poll_until_end(&mut feed, first_end);
    assert_eq!(
        events,
        [Event::Ended {
            reason: EXIT_3.to_owned()
        }]
    );
    assert_eq!(start_count(&starts), 1);

    assert!(feed
        .poll(first_end + FIRST_RETRY_DELAY - JUST_BEFORE)
        .is_empty());
    assert_eq!(start_count(&starts), 1);
    let second_end = first_end + FIRST_RETRY_DELAY;
    poll_until_end(&mut feed, second_end);
    assert_eq!(start_count(&starts), 2);

    let doubled = FIRST_RETRY_DELAY * 2;
    assert!(feed.poll(second_end + doubled - JUST_BEFORE).is_empty());
    assert_eq!(start_count(&starts), 2);
    poll_until_end(&mut feed, second_end + doubled);
    assert_eq!(start_count(&starts), 3);
}

#[test]
fn a_sample_sets_the_delay_of_the_feed_back() {
    let scratch = Scratch::new();
    let starts = scratch.path("starts");
    let fake = scratch.script(
        "ssh",
        &format!(
            "{}\nif [ $(( $(wc -l < {}) )) -ge 2 ]; then\n  exec {} --stream --count 1 --interval {INTERVAL}\nfi\nexit 3",
            count_start(&starts),
            quoted(&starts),
            glance()
        ),
    );
    warm();
    let mut feed = Feed::new(&fake, remote(), interval());
    let first_end = Instant::now();
    poll_until_end(&mut feed, first_end);

    let second_end = first_end + FIRST_RETRY_DELAY;
    let events = poll_until_end(&mut feed, second_end);
    assert_eq!(sample_count(&events), 1, "{events:?}");

    // Without the reset, the delay after the second end is 10 seconds.
    assert!(feed
        .poll(second_end + FIRST_RETRY_DELAY - JUST_BEFORE)
        .is_empty());
    poll_until_end(&mut feed, second_end + FIRST_RETRY_DELAY);
    assert_eq!(start_count(&starts), 3);
}

#[test]
fn a_program_that_does_not_start_ends_at_once_and_waits_the_backoff() {
    let scratch = Scratch::new();
    let missing = scratch.path("no-such-ssh");
    let mut feed = Feed::new(&missing, remote(), interval());
    let now = Instant::now();
    let events = feed.poll(now);
    assert_eq!(events.len(), 1, "{events:?}");
    let Event::Ended { reason } = &events[0] else {
        panic!("{events:?}");
    };
    assert!(
        reason.starts_with(&format!("cannot start {}: ", missing.display())),
        "{reason}"
    );
    assert!(feed.poll(now + FIRST_RETRY_DELAY - JUST_BEFORE).is_empty());
    assert_eq!(feed.poll(now + FIRST_RETRY_DELAY).len(), 1);
}

#[test]
fn a_stop_of_the_feed_kills_and_reaps_the_child() {
    let scratch = Scratch::new();
    let pid_file = scratch.path("pid.txt");
    let fake = scratch.script(
        "ssh",
        &format!(
            "echo $$ > {}\nexec {} --stream --count 240 --interval {INTERVAL}",
            quoted(&pid_file),
            glance()
        ),
    );
    warm();
    let mut feed = Feed::new(&fake, remote(), interval());
    let now = Instant::now();
    let deadline = now + EVENT_WAIT;
    while sample_count(&feed.poll(now)) == 0 {
        assert!(Instant::now() < deadline, "no sample within {EVENT_WAIT:?}");
        std::thread::sleep(POLL_PAUSE);
    }
    let pid = fs::read_to_string(&pid_file).expect("the pid file");
    feed.stop();
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
