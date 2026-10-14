//! The only module that starts `gh`.
//!
//! Every request to GitHub goes through the `gh` program, so `ha` needs no
//! token of its own. This module starts `gh`, reads what it prints, and maps
//! the answer to the types of the rest of the crate.
//!
//! `run_with_limit`, `RunFailure`, and `drain`, with their tests, come from
//! `src/cwt/src/pulls.rs`.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

/// Why `run_with_limit` gave no output.
#[derive(Debug)]
pub enum RunFailure {
    /// The program does not exist.
    NotInstalled,
    /// The program exists but the system could not start it.
    Spawn(std::io::Error),
    /// The program started but the wait for it failed.
    Wait(std::io::Error),
    /// The program did not finish in the time limit.
    TimedOut(Duration),
}

/// Runs `command` and waits at most `limit` for it to finish.
///
/// stdin is closed. A reader thread drains each of stdout and stderr, so a
/// chatty child cannot fill a pipe and block. The wait uses `wait_timeout`,
/// because a read to the end of the pipes would never finish for a hung child.
///
/// On a timeout the function kills and reaps the child and returns at once. It
/// does not join the reader threads. A hung child can leave a grandchild that
/// holds the write end of a pipe, and a join would block on the same stall that
/// the limit exists to escape. The threads end when the pipe closes.
///
/// # Errors
///
/// Returns a [`RunFailure`] when the program does not exist, cannot start,
/// cannot be waited for, or does not finish in `limit`.
pub fn run_with_limit(command: &mut Command, limit: Duration) -> Result<Output, RunFailure> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RunFailure::NotInstalled
            } else {
                RunFailure::Spawn(error)
            }
        })?;
    let stdout_reader = drain(child.stdout.take());
    let stderr_reader = drain(child.stderr.take());

    let status = match child.wait_timeout(limit) {
        Ok(Some(status)) => status,
        Ok(None) => {
            // Kill and reap the child. Leave the readers alone, see above.
            let _ = child.kill();
            let _ = child.wait();
            drop(stdout_reader);
            drop(stderr_reader);
            return Err(RunFailure::TimedOut(limit));
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(RunFailure::Wait(error));
        }
    };
    Ok(Output {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

/// Starts a thread that reads `pipe` to the end and returns the bytes.
///
/// A read error ends the read early and keeps the bytes that came before it.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

/// One answer of `gh api -i`, split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Response {
    /// The status code of the HTTP status line.
    status: u16,
    /// The reason phrase of the HTTP status line. It can be empty.
    reason: String,
    /// The headers, in the order and the case that `gh` printed them.
    headers: Vec<(String, String)>,
    /// Everything after the empty line that ends the headers.
    body: String,
}

impl Response {
    /// The value of the first header with this name. The name match ignores case.
    fn header(&self, name: &str) -> Option<&str> {
        let _ = name;
        None
    }
}

/// Splits the output of `gh api -i` into status, headers, and body.
///
/// Returns `None` when the first line is not an HTTP status line.
fn parse_response(text: &str) -> Option<Response> {
    let _ = text;
    None
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    use tempfile::TempDir;

    use super::*;

    /// A time limit that no stub that finishes comes near.
    const GENEROUS_LIMIT: Duration = Duration::from_secs(60);

    /// Writes an executable `#!/bin/sh` script into `dir` and returns its path.
    fn write_stub(dir: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn run_with_limit_returns_the_output_of_a_fast_program() {
        let dir = TempDir::new().unwrap();
        let stub = write_stub(&dir, "fast", "echo out; echo err >&2; exit 0");
        let output = run_with_limit(&mut Command::new(&stub), GENEROUS_LIMIT).unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
    }

    #[test]
    fn run_with_limit_reports_the_exit_status() {
        let dir = TempDir::new().unwrap();
        let stub = write_stub(&dir, "fails", "exit 3");
        let output = run_with_limit(&mut Command::new(&stub), GENEROUS_LIMIT).unwrap();
        assert_eq!(output.status.code(), Some(3));
    }

    #[test]
    fn run_with_limit_reports_a_missing_program() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let result = run_with_limit(&mut Command::new(&missing), GENEROUS_LIMIT);
        assert!(
            matches!(result, Err(RunFailure::NotInstalled)),
            "{result:?}"
        );
    }

    #[test]
    fn run_with_limit_reports_a_program_that_cannot_start() {
        let dir = TempDir::new().unwrap();
        let not_executable = dir.path().join("plain");
        std::fs::write(&not_executable, "text").unwrap();
        let result = run_with_limit(&mut Command::new(&not_executable), GENEROUS_LIMIT);
        assert!(matches!(result, Err(RunFailure::Spawn(_))), "{result:?}");
    }

    /// The output of `gh api -i` for a `200`: a status line that ends in a line
    /// feed, headers that end in CRLF, an empty line, and the body.
    fn ok_output(body: &str) -> String {
        format!(
            "HTTP/2.0 200 OK\n\
             Etag: W/\"4c00158b\"\r\n\
             X-Ratelimit-Limit: 5000\r\n\
             X-Ratelimit-Remaining: 4866\r\n\
             X-Ratelimit-Reset: 1791977652\r\n\
             \r\n{body}"
        )
    }

    #[test]
    fn a_response_splits_into_status_headers_and_body() {
        let response = parse_response(&ok_output("{\"a\":1}")).expect("a response");
        assert_eq!(response.status, 200);
        assert_eq!(response.reason, "OK");
        assert_eq!(response.body, "{\"a\":1}");
        assert_eq!(response.headers.len(), 4);
        assert_eq!(response.header("etag"), Some("W/\"4c00158b\""));
        assert_eq!(response.header("X-RATELIMIT-REMAINING"), Some("4866"));
        assert_eq!(response.header("x-ratelimit-reset"), Some("1791977652"));
        assert_eq!(response.header("missing"), None);
    }

    #[test]
    fn a_response_with_no_body_and_a_long_reason_parses() {
        let response =
            parse_response("HTTP/2.0 304 Not Modified\nEtag: \"x\"\r\n\r\n").expect("a response");
        assert_eq!(response.status, 304);
        assert_eq!(response.reason, "Not Modified");
        assert_eq!(response.body, "");
        assert_eq!(response.header("etag"), Some("\"x\""));
    }

    #[test]
    fn a_body_keeps_its_own_line_breaks_and_colons() {
        let response = parse_response("HTTP/2.0 200 OK\r\nA: b\r\n\r\n{\"x\":\r\n\"y: z\"}\n")
            .expect("a response");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "{\"x\":\r\n\"y: z\"}\n");
        assert_eq!(response.headers, [("A".to_owned(), "b".to_owned())]);
    }

    #[test]
    fn output_with_no_status_line_gives_none() {
        assert_eq!(parse_response(""), None);
        assert_eq!(parse_response("not http\r\n\r\nbody"), None);
        assert_eq!(parse_response("HTTP/2.0 abc OK\n\n"), None);
        assert_eq!(parse_response("{\"message\":\"x\"}"), None);
    }

    #[test]
    fn run_with_limit_gives_up_on_a_program_that_hangs() {
        let dir = TempDir::new().unwrap();
        let stub = write_stub(&dir, "hangs", "exec sleep 120");
        let limit = Duration::from_millis(500);
        let started = Instant::now();
        let result = run_with_limit(&mut Command::new(&stub), limit);
        assert!(started.elapsed() < Duration::from_secs(60));
        assert!(
            matches!(result, Err(RunFailure::TimedOut(reported)) if reported == limit),
            "{result:?}"
        );
    }
}
