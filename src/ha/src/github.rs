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

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::run::{Run, RunId, RunState};
use crate::status::RateLimit;
use crate::target::{OwnerName, RepoName};

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

/// Which runs of a repository to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunQuery {
    /// Every run created at or after the start of the UTC hour of this time.
    CreatedSince(DateTime<Utc>),
    /// Every run that is in progress or in the queue.
    Active,
}

/// Where `ha` gets its data. `GhSource` is the real one.
pub trait Source: Send + Sync {
    /// Lists the repositories of an owner. Archived repositories are left out.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when the list cannot be fetched.
    fn repos_of(&self, owner: &OwnerName) -> Result<Vec<RepoName>, SourceError>;

    /// Lists the workflow runs of a repository that match `query`.
    ///
    /// # Errors
    ///
    /// Returns a [`SourceError`] when a page of runs cannot be fetched.
    fn runs_of(&self, repo: &RepoName, query: RunQuery) -> Result<Vec<Run>, SourceError>;

    /// The rate limit that the last response stated. `None` before any response.
    fn rate_limit(&self) -> Option<RateLimit>;
}

/// A `Source` that runs the `gh` program.
///
/// One value serves many threads. The `ETag` cache and the rate limit sit behind
/// mutexes. No lock is held while a child runs.
pub struct GhSource {
    program: OsString,
    limit: Duration,
    etags: Mutex<HashMap<String, (String, Page)>>,
    rate: Mutex<Option<RateLimit>>,
}

impl GhSource {
    /// A source that runs `gh` from the path. Each child gets 30 seconds.
    #[must_use]
    pub fn new() -> GhSource {
        GhSource::with_program("gh", Duration::from_secs(30))
    }

    /// Runs `gh` with `args` and returns what it printed.
    fn run_gh(&self, args: &[&str]) -> Result<Output, SourceError> {
        let mut command = Command::new(&self.program);
        gitscratch::shed_inherited_git_environment_keeping_user_intent(&mut command);
        command
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .args(args);
        run_with_limit(&mut command, self.limit)
            .map_err(|failure| SourceError::Failed(format!("{failure:?}")))
    }

    /// Fetches `path` with `gh api -i` and splits the answer.
    ///
    /// With an `etag`, the request carries an `If-None-Match` header.
    fn request(&self, path: &str, etag: Option<&str>) -> Result<Response, SourceError> {
        let condition = etag.map(|etag| format!("If-None-Match: {etag}"));
        let mut args = vec!["api", "-i"];
        if let Some(condition) = &condition {
            args.extend(["-H", condition]);
        }
        args.push(path);
        let output = self.run_gh(&args)?;
        let text = String::from_utf8_lossy(&output.stdout);
        parse_response(&text).ok_or_else(|| SourceError::Failed(first_line(&output.stderr)))
    }

    /// Fetches the page `number` of the runs of `repo` for `filter`.
    fn runs_page(&self, repo: &RepoName, filter: &str, number: u32) -> Result<Page, SourceError> {
        let path = format!(
            "repos/{}/{}/actions/runs?per_page={PAGE_SIZE}&page={number}&{filter}",
            repo.owner(),
            repo.name()
        );
        let etag = self.lock_etags().get(&path).map(|(etag, _)| etag.clone());
        let response = self.request(&path, etag.as_deref())?;
        if response.status == 304 {
            return self
                .lock_etags()
                .get(&path)
                .map(|(_, page)| page.clone())
                .ok_or_else(|| {
                    SourceError::BadResponse(
                        "GitHub said 304 for a page that ha has not seen".to_owned(),
                    )
                });
        }
        let page = parse_runs(&response.body, repo)?;
        if let Some(etag) = response.header("etag") {
            self.lock_etags()
                .insert(path, (etag.to_owned(), page.clone()));
        }
        Ok(page)
    }

    /// Fetches every page of the runs of `repo` for `filter`.
    ///
    /// The fetch goes on while the last page was full and the count so far is
    /// below `total_count`. It stops after `MAX_PAGES` pages.
    fn all_runs(&self, repo: &RepoName, filter: &str) -> Result<Vec<Run>, SourceError> {
        let mut runs = Vec::new();
        for number in 1..=MAX_PAGES {
            let page = self.runs_page(repo, filter, number)?;
            let full = page.runs.len() >= PAGE_SIZE;
            runs.extend(page.runs);
            if !full || (runs.len() as u64) >= page.total_count {
                break;
            }
        }
        Ok(runs)
    }

    /// The `ETag` cache. The guard lives for one statement. A poisoned lock still
    /// holds a usable map, thus the poison is ignored.
    fn lock_etags(&self) -> std::sync::MutexGuard<'_, HashMap<String, (String, Page)>> {
        self.etags
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A source that runs `program`, and gives each child at most `limit`.
    #[must_use]
    pub fn with_program(program: impl Into<OsString>, limit: Duration) -> GhSource {
        GhSource {
            program: program.into(),
            limit,
            etags: Mutex::new(HashMap::new()),
            rate: Mutex::new(None),
        }
    }
}

impl Default for GhSource {
    fn default() -> Self {
        GhSource::new()
    }
}

impl Source for GhSource {
    fn repos_of(&self, owner: &OwnerName) -> Result<Vec<RepoName>, SourceError> {
        let _ = owner;
        Ok(Vec::new())
    }

    fn runs_of(&self, repo: &RepoName, query: RunQuery) -> Result<Vec<Run>, SourceError> {
        match query {
            RunQuery::CreatedSince(since) => {
                let filter = format!("created=%3E%3D{}", hour_start(since));
                self.all_runs(repo, &filter)
            }
            RunQuery::Active => Ok(Vec::new()),
        }
    }

    fn rate_limit(&self) -> Option<RateLimit> {
        None
    }
}

/// The number of runs in one full page. GitHub allows 100 at most.
const PAGE_SIZE: usize = 100;

/// The most pages that one query fetches. A query reaches 1000 runs at most.
const MAX_PAGES: u32 = 10;

/// The start of the UTC hour of `time`, as GitHub writes a time.
///
/// The text stays the same for one hour. The path of a request thus stays the
/// same between refreshes, and the `ETag` of GitHub can match.
fn hour_start(time: DateTime<Utc>) -> String {
    let seconds = time.timestamp().div_euclid(3600) * 3600;
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .unwrap_or(time)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// The first non-empty line of `bytes`, trimmed. The text is empty when none.
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
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

    /// A stub `gh` in a temporary directory. The script logs its arguments to
    /// `args.log` and then runs the `case` body that the test gives. The body can
    /// use `$dir`, the directory of the stub, to `cat` the canned outputs.
    struct Stub {
        dir: TempDir,
    }

    impl Stub {
        fn new(cases: &str) -> Stub {
            let dir = TempDir::new().unwrap();
            let body = format!(
                "dir=$(dirname \"$0\")\n{{ printf '%s\\n' \"$@\"; echo '--'; }} >> \"$dir/args.log\"\n{cases}"
            );
            write_stub(&dir, "gh", &body);
            Stub { dir }
        }

        /// Writes a canned output file next to the stub.
        fn file(&self, name: &str, content: &str) {
            std::fs::write(self.dir.path().join(name), content).unwrap();
        }

        fn source(&self) -> GhSource {
            GhSource::with_program(self.dir.path().join("gh"), GENEROUS_LIMIT)
        }

        /// The arguments of each call, in order.
        fn requests(&self) -> Vec<Vec<String>> {
            let log = std::fs::read_to_string(self.dir.path().join("args.log")).unwrap_or_default();
            let mut calls = Vec::new();
            let mut current = Vec::new();
            for line in log.lines() {
                if line == "--" {
                    calls.push(std::mem::take(&mut current));
                } else {
                    current.push(line.to_owned());
                }
            }
            calls
        }
    }

    #[test]
    fn a_created_since_query_asks_for_one_hour_aligned_page() {
        let stub = Stub::new("cat \"$dir/page1.out\"");
        stub.file("page1.out", &ok_output(RUNS_JSON));
        let tools = repo("timmattison/tools");
        let query = RunQuery::CreatedSince(time("2026-09-24T21:56:49Z"));
        let runs = stub.source().runs_of(&tools, query).expect("runs");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, RunId(36_064_582_280));
        assert_eq!(
            stub.requests(),
            [[
                "api",
                "-i",
                "repos/timmattison/tools/actions/runs?per_page=100&page=1&created=%3E%3D2026-09-24T21:00:00Z"
            ]]
        );
    }

    /// A runs body with `count` runs numbered from `first_id`, and `total` as the count.
    fn runs_json(total: u64, first_id: u64, count: u64) -> String {
        let runs: Vec<String> = (first_id..first_id + count)
            .map(|id| {
                format!(
                    r#"{{"id":{id},"name":"CI","display_title":"t","head_branch":"main","status":"completed","conclusion":"success","run_started_at":"2026-09-24T21:56:49Z","created_at":"2026-09-24T21:56:49Z","updated_at":"2026-09-24T21:57:56Z","html_url":"https://github.com/o/r/actions/runs/{id}"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"total_count":{total},"workflow_runs":[{}]}}"#,
            runs.join(",")
        )
    }

    const PAGE_CASES: &str = "case \"$*\" in
  *'&page=1&'*) cat \"$dir/page1.out\";;
  *'&page=2&'*) cat \"$dir/page2.out\";;
 esac";

    fn since_query() -> RunQuery {
        RunQuery::CreatedSince(time("2026-09-24T21:56:49Z"))
    }

    #[test]
    fn a_second_page_is_fetched_while_the_count_is_below_the_total() {
        let stub = Stub::new(PAGE_CASES);
        stub.file("page1.out", &ok_output(&runs_json(150, 1, 100)));
        stub.file("page2.out", &ok_output(&runs_json(150, 101, 50)));
        let runs = stub
            .source()
            .runs_of(&repo("o/r"), since_query())
            .expect("runs");
        assert_eq!(runs.len(), 150);
        assert_eq!(runs[149].id, RunId(150));
        assert_eq!(stub.requests().len(), 2);
    }

    #[test]
    fn a_page_with_fewer_than_100_runs_ends_the_fetch() {
        let stub = Stub::new(PAGE_CASES);
        stub.file("page1.out", &ok_output(&runs_json(500, 1, 99)));
        stub.file("page2.out", &ok_output(&runs_json(500, 100, 99)));
        let runs = stub
            .source()
            .runs_of(&repo("o/r"), since_query())
            .expect("runs");
        assert_eq!(runs.len(), 99);
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn the_fetch_ends_at_the_total_count() {
        let stub = Stub::new(PAGE_CASES);
        stub.file("page1.out", &ok_output(&runs_json(100, 1, 100)));
        stub.file("page2.out", &ok_output(&runs_json(100, 101, 100)));
        let runs = stub
            .source()
            .runs_of(&repo("o/r"), since_query())
            .expect("runs");
        assert_eq!(runs.len(), 100);
        assert_eq!(stub.requests().len(), 1);
    }

    #[test]
    fn the_fetch_stops_after_ten_pages() {
        let stub = Stub::new("cat \"$dir/page1.out\"");
        stub.file("page1.out", &ok_output(&runs_json(5000, 1, 100)));
        let runs = stub
            .source()
            .runs_of(&repo("o/r"), since_query())
            .expect("runs");
        assert_eq!(runs.len(), 1000);
        assert_eq!(stub.requests().len(), 10);
        let last = stub.requests().pop().expect("a request");
        assert!(last[2].contains("&page=10&"), "{last:?}");
    }

    const NOT_MODIFIED: &str = "HTTP/2.0 304 Not Modified\n\
         Etag: \"4c00158b\"\r\n\
         X-Ratelimit-Limit: 5000\r\n\
         X-Ratelimit-Remaining: 4866\r\n\
         X-Ratelimit-Reset: 1791977652\r\n\
         \r\n";

    /// A stub that answers `304` with exit status 1 when it gets an `ETag`.
    const ETAG_CASES: &str = "case \"$*\" in
  *If-None-Match*) cat \"$dir/not-modified.out\"; echo 'gh: HTTP 304' >&2; exit 1;;
  *) cat \"$dir/page1.out\";;
 esac";

    #[test]
    fn a_second_query_sends_the_etag_and_reuses_the_page_on_304() {
        let stub = Stub::new(ETAG_CASES);
        stub.file("page1.out", &ok_output(RUNS_JSON));
        stub.file("not-modified.out", NOT_MODIFIED);
        let source = stub.source();
        let tools = repo("timmattison/tools");
        let first = source.runs_of(&tools, since_query());
        assert_eq!(first.as_ref().map(Vec::len), Ok(2));
        let later = RunQuery::CreatedSince(time("2026-09-24T21:59:59Z"));
        let second = source.runs_of(&tools, later);
        assert_eq!(second, first);
        let requests = stub.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1][..4],
            ["api", "-i", "-H", "If-None-Match: W/\"4c00158b\""]
        );
        assert_eq!(requests[1][4], requests[0][2]);
    }

    #[test]
    fn a_query_for_another_hour_sends_no_etag() {
        let stub = Stub::new(ETAG_CASES);
        stub.file("page1.out", &ok_output(RUNS_JSON));
        stub.file("not-modified.out", NOT_MODIFIED);
        let source = stub.source();
        let tools = repo("timmattison/tools");
        source.runs_of(&tools, since_query()).expect("runs");
        let next_hour = RunQuery::CreatedSince(time("2026-09-24T22:00:00Z"));
        source.runs_of(&tools, next_hour).expect("runs");
        assert!(!stub.requests()[1].contains(&"-H".to_owned()));
    }

    #[test]
    fn a_304_with_no_cached_page_is_a_bad_response() {
        let stub = Stub::new("cat \"$dir/not-modified.out\"; exit 1");
        stub.file("not-modified.out", NOT_MODIFIED);
        let result = stub.source().runs_of(&repo("o/r"), since_query());
        assert!(
            matches!(result, Err(SourceError::BadResponse(_))),
            "{result:?}"
        );
    }

    /// Like `ok_output`, with the rate limit headers set by the caller.
    fn rated_output(body: &str, remaining: u64, reset: i64) -> String {
        format!(
            "HTTP/2.0 200 OK\n\
             Etag: W/\"e{remaining}\"\r\n\
             X-Ratelimit-Limit: 5000\r\n\
             X-Ratelimit-Remaining: {remaining}\r\n\
             X-Ratelimit-Reset: {reset}\r\n\
             \r\n{body}"
        )
    }

    fn rate(remaining: u64, reset: i64) -> RateLimit {
        RateLimit {
            limit: 5000,
            remaining,
            reset: DateTime::from_timestamp(reset, 0).expect("a valid time"),
        }
    }

    /// A stub with one canned page for the 21:00 hour and one for the 22:00 hour.
    fn hour_stub(first: &str, second: &str) -> Stub {
        let stub = Stub::new(
            "case \"$*\" in
  *'T21:'*) cat \"$dir/page1.out\";;
  *'T22:'*) cat \"$dir/page2.out\";;
 esac",
        );
        stub.file("page1.out", first);
        stub.file("page2.out", second);
        stub
    }

    fn ask_both_hours(source: &GhSource) {
        let tools = repo("o/r");
        source.runs_of(&tools, since_query()).expect("runs");
        let next = RunQuery::CreatedSince(time("2026-09-24T22:10:00Z"));
        source.runs_of(&tools, next).expect("runs");
    }

    #[test]
    fn the_rate_limit_is_none_before_any_request() {
        let stub = Stub::new("exit 1");
        assert_eq!(stub.source().rate_limit(), None);
    }

    #[test]
    fn the_rate_limit_comes_from_the_headers_of_a_response() {
        let stub = Stub::new("cat \"$dir/page1.out\"");
        stub.file("page1.out", &ok_output(RUNS_JSON));
        let source = stub.source();
        source.runs_of(&repo("o/r"), since_query()).expect("runs");
        assert_eq!(source.rate_limit(), Some(rate(4866, 1_791_977_652)));
    }

    #[test]
    fn a_304_updates_the_rate_limit_too() {
        let stub = Stub::new(ETAG_CASES);
        stub.file("page1.out", &ok_output(RUNS_JSON));
        stub.file(
            "not-modified.out",
            &NOT_MODIFIED.replace("Remaining: 4866", "Remaining: 4800"),
        );
        let source = stub.source();
        let tools = repo("o/r");
        source.runs_of(&tools, since_query()).expect("runs");
        source.runs_of(&tools, since_query()).expect("runs");
        assert_eq!(source.rate_limit(), Some(rate(4800, 1_791_977_652)));
    }

    #[test]
    fn a_later_reset_wins_even_with_more_requests_remaining() {
        let stub = hour_stub(
            &rated_output(&runs_json(0, 1, 0), 100, 1_000),
            &rated_output(&runs_json(0, 1, 0), 4999, 4_600),
        );
        let source = stub.source();
        ask_both_hours(&source);
        assert_eq!(source.rate_limit(), Some(rate(4999, 4_600)));
    }

    #[test]
    fn an_earlier_reset_does_not_replace_a_later_one() {
        let stub = hour_stub(
            &rated_output(&runs_json(0, 1, 0), 4999, 4_600),
            &rated_output(&runs_json(0, 1, 0), 100, 1_000),
        );
        let source = stub.source();
        ask_both_hours(&source);
        assert_eq!(source.rate_limit(), Some(rate(4999, 4_600)));
    }

    #[test]
    fn with_the_same_reset_the_lower_remaining_wins() {
        for (first, second) in [(4000, 3000), (3000, 4000)] {
            let stub = hour_stub(
                &rated_output(&runs_json(0, 1, 0), first, 4_600),
                &rated_output(&runs_json(0, 1, 0), second, 4_600),
            );
            let source = stub.source();
            ask_both_hours(&source);
            assert_eq!(source.rate_limit(), Some(rate(3000, 4_600)));
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
