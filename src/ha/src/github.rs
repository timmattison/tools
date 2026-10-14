//! The only module that starts `gh`.
//!
//! Every request to GitHub goes through the `gh` program, so `ha` needs no
//! token of its own. This module starts `gh`, reads what it prints, and maps
//! the answer to the types of the rest of the crate.
//!
//! `run_with_limit`, `RunFailure`, and `drain`, with their tests, come from
//! `src/cwt/src/pulls.rs`.

#![allow(
    dead_code,
    reason = "the parts of this module are joined in the next steps"
)]

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::run::{Run, RunId, RunState};
use crate::target::RepoName;

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

/// Why a request to GitHub gave no answer that `ha` can use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// The `gh` program is not on the path.
    #[error("gh is not installed: install it from https://cli.github.com")]
    NotInstalled,
    /// `gh` has no login, or GitHub refused the token. The text is the detail.
    #[error("gh is not logged in: run gh auth login")]
    NotLoggedIn(String),
    /// `gh` did not finish in the time limit.
    #[error("gh did not finish within {0:?}")]
    TimedOut(Duration),
    /// GitHub answered with a status of 400 or more.
    #[error("GitHub answered {status}: {message}")]
    Http {
        /// The HTTP status code.
        status: u16,
        /// The `message` of the JSON body, or the reason phrase.
        message: String,
    },
    /// `gh` ended with an error and no HTTP answer. The text is one line of stderr.
    #[error("gh failed: {0}")]
    Failed(String),
    /// `gh` printed an answer that `ha` cannot read. The text says why.
    #[error("gh gave an answer that ha cannot read: {0}")]
    BadResponse(String),
}

/// One page of workflow runs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Page {
    /// The `total_count` that GitHub states for the whole query.
    total_count: u64,
    /// The runs of this page.
    runs: Vec<Run>,
}

/// The JSON body of one page, as GitHub writes it.
#[derive(Deserialize)]
struct RunsBody {
    total_count: u64,
    workflow_runs: Vec<RunJson>,
}

/// One element of `workflow_runs`. Only the fields that `ha` reads are here.
#[derive(Deserialize)]
struct RunJson {
    id: u64,
    name: Option<String>,
    display_title: Option<String>,
    head_branch: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    run_started_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    html_url: String,
}

/// Maps the JSON body of one page of runs to a [`Page`].
///
/// `Run::repo` is `repo`, the repository that the request named.
fn parse_runs(body: &str, repo: &RepoName) -> Result<Page, SourceError> {
    let parsed: RunsBody =
        serde_json::from_str(body).map_err(|error| SourceError::BadResponse(error.to_string()))?;
    let runs = parsed
        .workflow_runs
        .into_iter()
        .map(|json| {
            let workflow = json.name.unwrap_or_default();
            let status = json.status.unwrap_or_default();
            Run {
                id: RunId(json.id),
                repo: repo.clone(),
                state: RunState::from_github(&status, json.conclusion.as_deref()),
                title: json.display_title.unwrap_or_else(|| workflow.clone()),
                workflow,
                branch: json.head_branch.unwrap_or_default(),
                started_at: json.run_started_at.unwrap_or(json.created_at),
                updated_at: json.updated_at,
                url: json.html_url,
            }
        })
        .collect();
    Ok(Page {
        total_count: parsed.total_count,
        runs,
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
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Splits `text` at the first empty line. The first part is the status line
/// and the headers, line by line. The second part is the body. A text with no
/// empty line has an empty body.
fn split_head(text: &str) -> (Vec<&str>, &str) {
    let mut head = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
        rest = after;
        if line.strip_suffix('\r').unwrap_or(line).is_empty() {
            return (head, rest);
        }
        head.push(line);
    }
    (head, "")
}

/// Splits the output of `gh api -i` into status, headers, and body.
///
/// Returns `None` when the first line is not an HTTP status line.
fn parse_response(text: &str) -> Option<Response> {
    let (head, body) = split_head(text);
    let mut lines = head
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    let status_line = lines.next()?;
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    let status = parts.next()?.parse().ok()?;
    let reason = parts.next().unwrap_or_default().trim().to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Some(Response {
        status,
        reason,
        headers,
        body: body.to_owned(),
    })
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

    fn repo(text: &str) -> RepoName {
        RepoName::parse(text).expect("a valid repository")
    }

    fn time(text: &str) -> DateTime<Utc> {
        text.parse().expect("a valid time")
    }

    const RUNS_JSON: &str = r#"{"total_count":2,"workflow_runs":[
        {"id":36064582280,"name":"CI","display_title":"Fix the thing","head_branch":"main",
         "status":"completed","conclusion":"failure","run_started_at":"2026-09-24T21:56:49Z",
         "created_at":"2026-09-24T21:50:00Z","updated_at":"2026-09-24T21:57:56Z",
         "html_url":"https://github.com/timmattison/tools/actions/runs/36064582280",
         "repository":{"full_name":"other/place"}},
        {"id":7,"name":"Release","display_title":null,"head_branch":null,
         "status":"in_progress","conclusion":null,"run_started_at":null,
         "created_at":"2026-09-25T01:02:03Z","updated_at":"2026-09-25T01:03:00Z",
         "html_url":"https://github.com/timmattison/tools/actions/runs/7"}]}"#;

    #[test]
    fn the_runs_json_maps_to_runs() {
        let tools = repo("timmattison/tools");
        let page = parse_runs(RUNS_JSON, &tools).expect("a page");
        assert_eq!(page.total_count, 2);
        assert_eq!(
            page.runs,
            [
                Run {
                    id: RunId(36_064_582_280),
                    repo: tools.clone(),
                    workflow: "CI".to_owned(),
                    branch: "main".to_owned(),
                    title: "Fix the thing".to_owned(),
                    state: RunState::Failed,
                    started_at: time("2026-09-24T21:56:49Z"),
                    updated_at: time("2026-09-24T21:57:56Z"),
                    url: "https://github.com/timmattison/tools/actions/runs/36064582280".to_owned(),
                },
                Run {
                    id: RunId(7),
                    repo: tools,
                    workflow: "Release".to_owned(),
                    branch: String::new(),
                    title: "Release".to_owned(),
                    state: RunState::Running,
                    started_at: time("2026-09-25T01:02:03Z"),
                    updated_at: time("2026-09-25T01:03:00Z"),
                    url: "https://github.com/timmattison/tools/actions/runs/7".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn a_run_takes_the_repository_that_the_request_named() {
        let asked = repo("Timmattison/Tools");
        let page = parse_runs(RUNS_JSON, &asked).expect("a page");
        assert_eq!(page.runs.len(), 2);
        assert!(page
            .runs
            .iter()
            .all(|run| run.repo.to_string() == "Timmattison/Tools"));
    }

    #[test]
    fn invalid_json_is_a_bad_response() {
        let tools = repo("timmattison/tools");
        for body in ["", "not json", "{}", "{\"total_count\":1}", "[]"] {
            let result = parse_runs(body, &tools);
            assert!(
                matches!(result, Err(SourceError::BadResponse(_))),
                "{body:?} gave {result:?}"
            );
        }
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
