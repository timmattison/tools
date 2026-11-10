//! One lookup of the open pull requests of a branch.
//!
//! The lookup is one REST read through `gh`. This file builds the request, runs
//! `gh` with a time limit, and reads the reply into a [`LookupOutcome`]. It
//! keeps no state. The cache per target, the one lookup in flight, and the
//! block after a rate limit belong to the caller.
//!
//! Every rule about the reply lives in [`interpret`], which is a pure function
//! of text. A test of a rule needs no child process.

use std::ffi::OsStr;
use std::process::Command;
use std::time::{Duration, SystemTime};

use ghrest::{parse_response, EtagEntry, PullNumber, RateLimitWait, Response};
use gitscratch::InheritedGitEnvironment;
use runlimit::{run_with_limit, RunFailure};
use serde::Deserialize;

use super::PullTarget;

/// The longest time that one run of `gh` can take. A run past the bound is a failure.
pub(crate) const LOOKUP_LIMIT: Duration = Duration::from_secs(10);

/// The wait after a rate limit reply that states no wait.
pub(crate) const RATE_LIMIT_FALLBACK: Duration = Duration::from_secs(60);

/// The variable that makes `gh` act as if it wrote to a terminal. It puts color codes in the reply.
const GH_FORCE_TTY: &str = "GH_FORCE_TTY";

/// The variable that forces color output of tools that read it. It puts color codes in the reply.
const CLICOLOR_FORCE: &str = "CLICOLOR_FORCE";

/// The variable that stops `gh` from asking questions.
const GH_PROMPT_DISABLED: &str = "GH_PROMPT_DISABLED";

/// The variable that stops the update check of `gh`.
const GH_NO_UPDATE_NOTIFIER: &str = "GH_NO_UPDATE_NOTIFIER";

/// The value that switches on one of the variables above.
const SWITCH_ON: &str = "1";

/// The number of pull requests that one page of the lookup holds.
const PER_PAGE: u32 = 100;

/// What one lookup found out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LookupOutcome {
    /// GitHub answered. The numbers are the open pull requests of the branch.
    Answered {
        /// The numbers in ascending order, with no repeats.
        numbers: Vec<PullNumber>,
        /// The cache entry to keep for the next request. It is the new entry
        /// after a `200` with an `ETag`, the old entry after a `304`, and
        /// `None` after a `200` with no `ETag`.
        entry: Option<EtagEntry>,
    },
    /// GitHub refused because of a rate limit. The caller sends no request for this long.
    RateLimited(Duration),
    /// The lookup failed. The text is one line that says why. The indicator shows `PR ?`.
    Failed(String),
    /// The `gh` program does not exist.
    NotInstalled,
}

/// Percent-encodes `text` for the value of a query parameter.
///
/// The bytes `A-Z a-z 0-9 - . _ ~` stay as they are. Every other byte of the
/// UTF-8 form becomes `%XX` with uppercase hex. A `+` becomes `%2B`, because a
/// `+` in a query means a space.
fn percent_encode(text: &str) -> String {
    text.to_owned()
}

/// The path of the request, for example
/// `repos/o/r/pulls?head=o:feat%2Fx&state=open&per_page=100`.
///
/// The head is `<owner>:<branch>`. GitHub ignores a head with no owner and
/// gives every open pull request of the repository. The owner and the name
/// are in the path itself, never as `{owner}/{repo}`, and `GithubRepo` has
/// checked both. They hold only letters, digits, `-`, `_`, and `.`.
pub(crate) fn pulls_path(target: &PullTarget) -> String {
    let repo = target.repo();
    format!(
        "repos/{owner}/{name}/pulls?head={head_owner}:{branch}&state=open&per_page={PER_PAGE}",
        owner = repo.owner(),
        name = repo.name(),
        head_owner = percent_encode(repo.owner()),
        branch = percent_encode(target.head()),
    )
}

/// Sets the environment of the child that runs `gh`.
///
/// `inherited_keys` names the variables that the child inherits. Production
/// passes the names of the environment of this process. A test passes its own
/// names, so no test sets a real `GIT_` variable. Cargo runs the tests of one
/// binary on many threads, and such a variable would reach all of them.
///
/// - Each `GIT_` variable leaves, except the six that a person sets on purpose.
/// - `GH_FORCE_TTY` and `CLICOLOR_FORCE` leave, because they put color codes in the reply.
/// - `GH_PROMPT_DISABLED=1` stops each question of `gh`.
/// - `GH_NO_UPDATE_NOTIFIER=1` stops the update check of `gh`. That check is a
///   second network call on each poll.
/// - `GH_TOKEN`, `GITHUB_TOKEN`, and `GH_ENTERPRISE_TOKEN` stay as they are.
fn configure_child<I, K>(command: &mut Command, inherited_keys: I)
where
    I: IntoIterator<Item = K>,
    K: AsRef<OsStr>,
{
    let _ = (command, inherited_keys);
}

/// Builds the command of one lookup, with `keys` as the names of the inherited environment.
fn lookup_command_with<I, K>(
    program: &OsStr,
    target: &PullTarget,
    condition: Option<&str>,
    keys: I,
) -> Command
where
    I: IntoIterator<Item = K>,
    K: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command
        .args(["api", "-i", "--hostname"])
        .arg(target.repo().host());
    if let Some(condition) = condition {
        command.args(["-H", condition]);
    }
    command.arg(pulls_path(target));
    configure_child(&mut command, keys);
    command
}

/// Builds the command of one lookup. `condition` is a whole `If-None-Match: <etag>` line.
fn lookup_command(program: &OsStr, target: &PullTarget, condition: Option<&str>) -> Command {
    lookup_command_with(
        program,
        target,
        condition,
        std::env::vars_os().map(|(key, _)| key),
    )
}

/// One entry of the page that GitHub answers with.
#[derive(Deserialize)]
struct PullEntry {
    number: PullNumber,
    head: HeadEntry,
}

/// The head of one entry.
#[derive(Deserialize)]
struct HeadEntry {
    #[serde(rename = "ref")]
    branch: String,
}

/// Reads the numbers of the pull requests of `branch` out of a body.
///
/// An entry whose head branch is another branch does not count. GitHub gives
/// every open pull request when it ignores the `head` parameter, and this
/// check stops such a reply from showing the pull request of another branch.
/// The numbers come back in ascending order, with no repeats.
///
/// Returns `None` when the body is not a JSON array of such entries.
fn parse_numbers(body: &str, branch: &str) -> Option<Vec<PullNumber>> {
    let entries: Vec<PullEntry> = serde_json::from_str(body).ok()?;
    let mut numbers: Vec<PullNumber> = entries
        .into_iter()
        .filter(|entry| entry.head.branch == branch)
        .map(|entry| entry.number)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    Some(numbers)
}

/// The first line of `text` that holds more than white space.
fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

/// Builds the outcome of a reply that has a status line.
fn answer(
    response: Response,
    path: &str,
    target: &PullTarget,
    cached: Option<&EtagEntry>,
    now: SystemTime,
) -> LookupOutcome {
    let status = response.status();
    if status == 304 {
        let numbers = cached
            .and_then(|entry| entry.body_for(path))
            .and_then(|body| parse_numbers(body, target.head()));
        return match numbers {
            Some(numbers) => LookupOutcome::Answered {
                numbers,
                entry: cached.cloned(),
            },
            None => {
                LookupOutcome::Failed("GitHub answered 304 and no kept answer is usable".to_owned())
            }
        };
    }
    if (200..300).contains(&status) {
        return match parse_numbers(response.body(), target.head()) {
            Some(numbers) => LookupOutcome::Answered {
                numbers,
                entry: EtagEntry::from_response(path, response),
            },
            None => LookupOutcome::Failed(format!(
                "GitHub answered {status} with a body that is not a list of pull requests"
            )),
        };
    }
    match response.rate_limit_wait(now) {
        Some(RateLimitWait::Stated(wait)) => LookupOutcome::RateLimited(wait),
        Some(RateLimitWait::Unstated) => LookupOutcome::RateLimited(RATE_LIMIT_FALLBACK),
        None => LookupOutcome::Failed(format!("GitHub answered {status} {}", response.reason())),
    }
}

/// Reads the output of a finished `gh` into an outcome.
///
/// The status line of the reply decides. The exit status does not, because
/// `gh api` exits with 1 after a `304`. The exit status and the first line of
/// `stderr` only explain a run that printed no reply.
fn interpret(
    stdout: &str,
    stderr: &str,
    exit: Option<i32>,
    path: &str,
    target: &PullTarget,
    cached: Option<&EtagEntry>,
    now: SystemTime,
) -> LookupOutcome {
    let _ = (stdout, stderr, exit, path, target, cached, now);
    LookupOutcome::Failed(String::new())
}

/// Runs one lookup with `program` as the `gh` program.
///
/// `cached` is the entry that the caller kept. Its `ETag` goes out only when
/// the entry belongs to the same path. A run that lasts longer than `limit` is
/// a failure.
pub(crate) fn run_lookup(
    program: &OsStr,
    target: &PullTarget,
    cached: Option<&EtagEntry>,
    limit: Duration,
) -> LookupOutcome {
    let path = pulls_path(target);
    let condition = cached.and_then(|entry| entry.condition_for(&path));
    let mut command = lookup_command(program, target, condition.as_deref());
    match run_with_limit(&mut command, limit) {
        Ok(output) => interpret(
            &String::from_utf8_lossy(&output.stdout),
            &String::from_utf8_lossy(&output.stderr),
            output.status.code(),
            &path,
            target,
            cached,
            SystemTime::now(),
        ),
        Err(RunFailure::NotInstalled) => LookupOutcome::NotInstalled,
        Err(RunFailure::TimedOut(bound)) => {
            LookupOutcome::Failed(format!("gh ran past the bound of {bound:?}"))
        }
        Err(RunFailure::Spawn(error)) => {
            LookupOutcome::Failed(format!("gh did not start: {error}"))
        }
        Err(RunFailure::Wait(error)) => {
            LookupOutcome::Failed(format!("the wait for gh failed: {error}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Instant, UNIX_EPOCH};

    use ghrest::GithubRepo;
    use tempfile::TempDir;

    use super::*;

    /// A time limit that no stub that finishes comes near.
    const GENEROUS_LIMIT: Duration = Duration::from_secs(60);

    /// The instant that the pure tests use as the present.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1000)
    }

    fn target(branch: &str) -> PullTarget {
        let repo = GithubRepo::from_remote_url("https://github.com/octo/repo.git", None)
            .expect("a GitHub repository");
        PullTarget::new(repo, branch)
    }

    /// A body with one entry for each `(number, branch)` pair.
    fn body(pulls: &[(u64, &str)]) -> String {
        let items: Vec<String> = pulls
            .iter()
            .map(|(number, branch)| {
                format!("{{\"number\":{number},\"title\":\"t\",\"head\":{{\"ref\":\"{branch}\",\"sha\":\"abc\"}}}}")
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// The output of `gh api -i` for a reply with this status line, these
    /// header lines, and this body. The status line ends in a line feed and
    /// each header ends in CRLF, as `gh` prints them.
    fn reply(status: &str, headers: &[&str], body: &str) -> String {
        let headers: String = headers
            .iter()
            .map(|header| format!("{header}\r\n"))
            .collect();
        format!("HTTP/2.0 {status}\n{headers}\r\n{body}")
    }

    fn ok_reply(etag: Option<&str>, body: &str) -> String {
        let etag = etag.map(|etag| format!("Etag: {etag}"));
        let headers: Vec<&str> = etag.as_deref().into_iter().collect();
        reply("200 OK", &headers, body)
    }

    /// The entry that a `200` reply with this `ETag` leaves in the cache.
    fn entry_for(target: &PullTarget, etag: &str, body: &str) -> EtagEntry {
        let response = parse_response(&ok_reply(Some(etag), body)).expect("a response");
        EtagEntry::from_response(&pulls_path(target), response).expect("an entry")
    }

    fn interpret_reply(
        text: &str,
        target: &PullTarget,
        cached: Option<&EtagEntry>,
    ) -> LookupOutcome {
        interpret(
            text,
            "",
            Some(0),
            &pulls_path(target),
            target,
            cached,
            now(),
        )
    }

    fn numbers(values: &[u64]) -> Vec<PullNumber> {
        values.iter().map(|&value| PullNumber::new(value)).collect()
    }

    fn is_failed(outcome: &LookupOutcome) -> bool {
        matches!(outcome, LookupOutcome::Failed(reason) if !reason.is_empty())
    }

    // percent_encode and pulls_path

    #[test]
    fn percent_encode_keeps_the_unreserved_bytes() {
        assert_eq!(percent_encode("Az09-._~"), "Az09-._~");
    }

    #[test]
    fn percent_encode_writes_other_bytes_as_uppercase_hex() {
        let cases = [
            ("a+b", "a%2Bb"),
            ("feat/x", "feat%2Fx"),
            ("x#1", "x%231"),
            ("a b", "a%20b"),
            ("a%b", "a%25b"),
            ("a&b=c?d", "a%26b%3Dc%3Fd"),
            ("日本語", "%E6%97%A5%E6%9C%AC%E8%AA%9E"),
            ("🎉", "%F0%9F%8E%89"),
            ("café", "caf%C3%A9"),
            ("", ""),
        ];
        for (text, encoded) in cases {
            assert_eq!(percent_encode(text), encoded, "{text}");
        }
    }

    #[test]
    fn the_path_names_the_owner_and_the_name_and_pairs_the_head_with_the_owner() {
        assert_eq!(
            pulls_path(&target("main")),
            "repos/octo/repo/pulls?head=octo:main&state=open&per_page=100"
        );
    }

    #[test]
    fn the_path_encodes_the_branch() {
        let cases = [
            ("a+b", "a%2Bb"),
            ("feat/x", "feat%2Fx"),
            ("x#1", "x%231"),
            ("日本語", "%E6%97%A5%E6%9C%AC%E8%AA%9E"),
            ("🎉", "%F0%9F%8E%89"),
            ("café", "caf%C3%A9"),
        ];
        for (branch, encoded) in cases {
            assert_eq!(
                pulls_path(&target(branch)),
                format!("repos/octo/repo/pulls?head=octo:{encoded}&state=open&per_page=100"),
                "{branch}"
            );
        }
    }

    // rate_limit_wait through interpret, and the other statuses

    #[test]
    fn a_200_gives_the_sorted_numbers_of_the_branch_and_a_new_entry() {
        let target = target("feat/x");
        let page = body(&[(9, "feat/x"), (3, "other"), (4, "feat/x"), (9, "feat/x")]);
        let outcome = interpret_reply(&ok_reply(Some("W/\"e1\""), &page), &target, None);
        let expected = entry_for(&target, "W/\"e1\"", &page);
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: numbers(&[4, 9]),
                entry: Some(expected),
            }
        );
    }

    #[test]
    fn a_200_with_no_etag_keeps_no_entry() {
        let target = target("feat/x");
        let outcome = interpret_reply(&ok_reply(None, &body(&[(4, "feat/x")])), &target, None);
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: numbers(&[4]),
                entry: None,
            }
        );
    }

    #[test]
    fn a_200_with_an_empty_list_gives_no_numbers() {
        let target = target("feat/x");
        let outcome = interpret_reply(&ok_reply(Some("\"e\""), "[]"), &target, None);
        assert!(
            matches!(&outcome, LookupOutcome::Answered { numbers, entry: Some(_) } if numbers.is_empty()),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_200_that_lists_only_other_branches_gives_no_numbers() {
        let target = target("feat/x");
        let page = body(&[(1, "main"), (2, "feat")]);
        let outcome = interpret_reply(&ok_reply(None, &page), &target, None);
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: Vec::new(),
                entry: None,
            }
        );
    }

    #[test]
    fn a_200_with_a_body_that_is_not_a_list_fails() {
        let target = target("feat/x");
        for text in ["{\"message\":\"x\"}", "not json", "", "[{\"number\":1}]"] {
            let outcome = interpret_reply(&ok_reply(None, text), &target, None);
            assert!(is_failed(&outcome), "{text}: {outcome:?}");
        }
    }

    #[test]
    fn a_304_gives_the_kept_answer_and_the_old_entry_whatever_the_exit_status() {
        let target = target("feat/x");
        let kept = entry_for(&target, "\"e1\"", &body(&[(8, "feat/x"), (2, "feat/x")]));
        let text = reply("304 Not Modified", &["Etag: \"e1\""], "");
        let outcome = interpret(
            &text,
            "gh: HTTP 304\n",
            Some(1),
            &pulls_path(&target),
            &target,
            Some(&kept),
            now(),
        );
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: numbers(&[2, 8]),
                entry: Some(kept),
            }
        );
    }

    #[test]
    fn a_304_with_no_kept_answer_fails() {
        let target = target("feat/x");
        let text = reply("304 Not Modified", &[], "");
        assert!(is_failed(&interpret_reply(&text, &target, None)));
        let other = entry_for(&target, "\"e1\"", "[]");
        let other_target = PullTarget::new(
            GithubRepo::from_remote_url("https://github.com/octo/repo.git", None).unwrap(),
            "elsewhere",
        );
        assert!(is_failed(&interpret_reply(
            &text,
            &other_target,
            Some(&other)
        )));
    }

    #[test]
    fn a_403_with_an_empty_quota_and_a_reset_gives_the_wait_to_the_reset() {
        let text = reply(
            "403 Forbidden",
            &["X-Ratelimit-Remaining: 0", "X-Ratelimit-Reset: 1120"],
            "{\"message\":\"API rate limit exceeded\"}",
        );
        assert_eq!(
            interpret_reply(&text, &target("b"), None),
            LookupOutcome::RateLimited(Duration::from_secs(120))
        );
    }

    #[test]
    fn a_429_with_retry_after_gives_those_seconds() {
        let text = reply("429 Too Many Requests", &["Retry-After: 30"], "");
        assert_eq!(
            interpret_reply(&text, &target("b"), None),
            LookupOutcome::RateLimited(Duration::from_secs(30))
        );
    }

    #[test]
    fn a_rate_limit_with_no_wait_gives_the_fallback() {
        let text = reply("403 Forbidden", &["X-Ratelimit-Remaining: 0"], "");
        assert_eq!(
            interpret_reply(&text, &target("b"), None),
            LookupOutcome::RateLimited(RATE_LIMIT_FALLBACK)
        );
    }

    #[test]
    fn a_403_with_no_rate_limit_header_fails() {
        let text = reply(
            "403 Forbidden",
            &[],
            "{\"message\":\"Resource not accessible\"}",
        );
        assert_eq!(
            interpret_reply(&text, &target("b"), None),
            LookupOutcome::Failed("GitHub answered 403 Forbidden".to_owned())
        );
    }

    #[test]
    fn a_500_fails_and_says_the_status() {
        let text = reply("500 Internal Server Error", &[], "");
        assert_eq!(
            interpret_reply(&text, &target("b"), None),
            LookupOutcome::Failed("GitHub answered 500 Internal Server Error".to_owned())
        );
    }

    #[test]
    fn a_run_with_no_reply_fails_with_the_first_line_of_stderr() {
        let target = target("b");
        let outcome = interpret(
            "",
            "\nerror connecting to api.github.com\ncheck your internet connection\n",
            Some(1),
            &pulls_path(&target),
            &target,
            None,
            now(),
        );
        assert_eq!(
            outcome,
            LookupOutcome::Failed("error connecting to api.github.com".to_owned())
        );
    }

    #[test]
    fn a_run_with_no_reply_and_no_stderr_fails_with_the_exit_code() {
        let target = target("b");
        let outcome = interpret(
            "",
            "  \n",
            Some(4),
            &pulls_path(&target),
            &target,
            None,
            now(),
        );
        assert_eq!(
            outcome,
            LookupOutcome::Failed("gh printed no reply and exited with 4".to_owned())
        );
    }

    // run_lookup with a stub gh

    /// Writes an executable `#!/bin/sh` script into `dir` and returns its path.
    fn write_stub(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn args_file(dir: &TempDir) -> PathBuf {
        dir.path().join("args")
    }

    fn env_file(dir: &TempDir) -> PathBuf {
        dir.path().join("env")
    }

    /// A stub `gh` that records its arguments and its environment, prints
    /// `output` to stdout and `stderr` to stderr, and exits with `exit`.
    fn answering_stub(dir: &TempDir, output: &str, stderr: &str, exit: i32) -> PathBuf {
        let output_file = dir.path().join("output");
        std::fs::write(&output_file, output).unwrap();
        write_stub(
            dir,
            "gh-stub",
            &format!(
                "printf '%s\\n' \"$@\" > \"{args}\"\nenv > \"{env}\"\ncat \"{output}\"\nprintf '%s' '{stderr}' >&2\nexit {exit}",
                args = args_file(dir).display(),
                env = env_file(dir).display(),
                output = output_file.display(),
            ),
        )
    }

    fn recorded_args(dir: &TempDir) -> Vec<String> {
        std::fs::read_to_string(args_file(dir))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_stub_receives_the_request_with_an_encoded_branch() {
        for (branch, encoded) in [
            ("a+b", "a%2Bb"),
            ("feat/x", "feat%2Fx"),
            ("x#1", "x%231"),
            ("日本語", "%E6%97%A5%E6%9C%AC%E8%AA%9E"),
            ("🎉", "%F0%9F%8E%89"),
            ("café", "caf%C3%A9"),
        ] {
            let dir = TempDir::new().unwrap();
            let stub = answering_stub(&dir, &ok_reply(None, "[]"), "", 0);
            let outcome = run_lookup(stub.as_os_str(), &target(branch), None, GENEROUS_LIMIT);
            assert!(
                matches!(outcome, LookupOutcome::Answered { .. }),
                "{outcome:?}"
            );
            assert_eq!(
                recorded_args(&dir),
                [
                    "api".to_owned(),
                    "-i".to_owned(),
                    "--hostname".to_owned(),
                    "github.com".to_owned(),
                    format!("repos/octo/repo/pulls?head=octo:{encoded}&state=open&per_page=100"),
                ],
                "{branch}"
            );
        }
    }

    #[test]
    fn a_cached_entry_for_the_same_path_sends_if_none_match() {
        let dir = TempDir::new().unwrap();
        let target = target("feat/x");
        let kept = entry_for(&target, "W/\"abc\"", &body(&[(5, "feat/x")]));
        let stub = answering_stub(&dir, &reply("304 Not Modified", &[], ""), "gh: HTTP 304", 1);
        let outcome = run_lookup(stub.as_os_str(), &target, Some(&kept), GENEROUS_LIMIT);
        assert_eq!(
            recorded_args(&dir),
            [
                "api",
                "-i",
                "--hostname",
                "github.com",
                "-H",
                "If-None-Match: W/\"abc\"",
                pulls_path(&target).as_str(),
            ]
        );
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: numbers(&[5]),
                entry: Some(kept),
            }
        );
    }

    #[test]
    fn a_cached_entry_for_another_path_sends_no_condition() {
        let dir = TempDir::new().unwrap();
        let other = entry_for(&target("other"), "\"abc\"", "[]");
        let stub = answering_stub(&dir, &ok_reply(None, "[]"), "", 0);
        run_lookup(
            stub.as_os_str(),
            &target("feat/x"),
            Some(&other),
            GENEROUS_LIMIT,
        );
        assert!(!recorded_args(&dir).contains(&"-H".to_owned()));
    }

    #[test]
    fn a_stub_that_answers_304_and_exits_1_is_an_answer_not_a_failure() {
        let dir = TempDir::new().unwrap();
        let target = target("feat/x");
        let kept = entry_for(&target, "\"e\"", &body(&[(7, "feat/x")]));
        let stub = answering_stub(
            &dir,
            &reply("304 Not Modified", &["Etag: \"e\""], ""),
            "gh: HTTP 304",
            1,
        );
        let outcome = run_lookup(stub.as_os_str(), &target, Some(&kept), GENEROUS_LIMIT);
        assert!(
            matches!(&outcome, LookupOutcome::Answered { numbers: found, .. } if *found == numbers(&[7])),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_stub_that_answers_200_gives_the_numbers_and_an_entry() {
        let dir = TempDir::new().unwrap();
        let target = target("feat/x");
        let page = body(&[(9, "feat/x"), (3, "feat/x"), (6, "main")]);
        let stub = answering_stub(&dir, &ok_reply(Some("\"e2\""), &page), "", 0);
        let outcome = run_lookup(stub.as_os_str(), &target, None, GENEROUS_LIMIT);
        assert_eq!(
            outcome,
            LookupOutcome::Answered {
                numbers: numbers(&[3, 9]),
                entry: Some(entry_for(&target, "\"e2\"", &page)),
            }
        );
    }

    #[test]
    fn a_stub_that_prints_no_reply_fails() {
        let dir = TempDir::new().unwrap();
        let stub = answering_stub(&dir, "", "error connecting to api.github.com", 1);
        let outcome = run_lookup(stub.as_os_str(), &target("b"), None, GENEROUS_LIMIT);
        assert_eq!(
            outcome,
            LookupOutcome::Failed("error connecting to api.github.com".to_owned())
        );
    }

    #[test]
    fn a_stub_that_runs_past_the_bound_fails_and_the_call_returns_in_time() {
        let dir = TempDir::new().unwrap();
        let stub = write_stub(&dir, "gh-slow", "exec sleep 120");
        let started = Instant::now();
        let outcome = run_lookup(
            stub.as_os_str(),
            &target("b"),
            None,
            Duration::from_millis(300),
        );
        assert!(started.elapsed() < Duration::from_secs(60));
        assert!(
            matches!(&outcome, LookupOutcome::Failed(reason) if reason.contains("300ms")),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_program_that_does_not_exist_is_not_installed() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("no-such-gh");
        let outcome = run_lookup(missing.as_os_str(), &target("b"), None, GENEROUS_LIMIT);
        assert_eq!(outcome, LookupOutcome::NotInstalled);
    }

    // the environment of the child

    /// The value of `name` in the recorded environment, when it is there.
    fn recorded_env(path: &Path, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix(&prefix).map(str::to_owned))
    }

    #[test]
    fn the_child_sheds_git_color_and_prompt_variables_and_keeps_the_user_ones() {
        let dir = TempDir::new().unwrap();
        let stub = answering_stub(&dir, &ok_reply(None, "[]"), "", 0);
        let inherited = [
            ("GIT_DIR", "/decoy"),
            ("GIT_INDEX_FILE", "/decoy/index"),
            ("GIT_CONFIG_PARAMETERS", "'user.name=decoy'"),
            ("GIT_SSH_COMMAND", "ssh -i key"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GH_FORCE_TTY", "100%"),
            ("CLICOLOR_FORCE", "1"),
            ("GH_TOKEN", "token-value"),
        ];
        let mut command = Command::new(&stub);
        for (name, value) in inherited {
            command.env(name, value);
        }
        configure_child(&mut command, inherited.iter().map(|(name, _)| *name));
        let output = run_with_limit(&mut command, GENEROUS_LIMIT).expect("the stub runs");
        assert!(output.status.success());

        let env = env_file(&dir);
        for gone in [
            "GIT_DIR",
            "GIT_INDEX_FILE",
            "GIT_CONFIG_PARAMETERS",
            "GH_FORCE_TTY",
            "CLICOLOR_FORCE",
        ] {
            assert_eq!(recorded_env(&env, gone), None, "{gone}");
        }
        for (name, value) in [
            ("GIT_SSH_COMMAND", "ssh -i key"),
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GH_TOKEN", "token-value"),
            ("GH_PROMPT_DISABLED", "1"),
            ("GH_NO_UPDATE_NOTIFIER", "1"),
        ] {
            assert_eq!(recorded_env(&env, name).as_deref(), Some(value), "{name}");
        }
    }

    #[test]
    fn the_lookup_command_carries_the_same_environment_rules() {
        let command = lookup_command_with(
            OsStr::new("gh"),
            &target("b"),
            None,
            ["GIT_DIR", "GIT_SSH_COMMAND"],
        );
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&("GIT_DIR".to_owned(), None)));
        assert!(!envs.iter().any(|(key, _)| key == "GIT_SSH_COMMAND"));
        assert!(envs.contains(&("GH_PROMPT_DISABLED".to_owned(), Some("1".to_owned()))));
    }
}
