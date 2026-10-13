//! Open pull requests, read from the output of `gh pr list`.
//!
//! `cwt` shows the numbers of the open pull requests after the branch of a
//! worktree. One `gh pr list --json` call for each repository gives all the
//! data. This module holds the pure part of that work: it reads the URL of a
//! remote, and it reads the JSON that `gh` prints. It runs no command and it
//! needs no network, so every rule in it has an offline test.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt;

use serde::Deserialize;

/// The number of one pull request.
///
/// A newtype keeps a pull request number apart from other integers. It reads
/// from a bare JSON number and it displays with a leading `#`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(transparent)]
pub struct PullNumber(u64);

impl fmt::Display for PullNumber {
    /// Writes the number in the form that GitHub shows, for example `#547`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// The host that `gh` talks to when `GH_HOST` is not set.
const GITHUB_HOST: &str = "github.com";

/// The suffix that many remote URLs add to the name of a repository.
const GIT_SUFFIX: &str = ".git";

/// The URL schemes that name a remote on a network host.
const NETWORK_SCHEMES: [&str; 4] = ["ssh", "https", "http", "git"];

/// Splits the URL of a remote into its host and its path.
///
/// The URL form `scheme://[user@]host[:port]/path` and the scp-like form
/// `[user@]host:path` both work. The host keeps its original case and loses
/// the user and the port. Returns `None` for a scheme that is not a network
/// scheme, for a local path, and for a port that is not a number.
fn split_host_and_path(url: &str) -> Option<(&str, &str)> {
    if let Some((scheme, rest)) = url.split_once("://") {
        if !NETWORK_SCHEMES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(scheme))
        {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host_and_port = authority
            .rsplit_once('@')
            .map_or(authority, |(_, after)| after);
        return match host_and_port.split_once(':') {
            Some((host, port)) => (!port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
                .then_some((host, path)),
            None => Some((host_and_port, path)),
        };
    }
    let (before, path) = url.split_once(':')?;
    if before.contains('/') {
        return None;
    }
    let host = before.rsplit_once('@').map_or(before, |(_, after)| after);
    Some((host, path))
}

/// Tells if `part` is a valid owner or repository name.
///
/// The part is not empty and holds only ASCII letters, digits, `-`, `_`, and
/// `.`. A part of only `.git` is not a name, because the caller has already
/// removed one `.git` suffix and an empty name is left.
fn is_name_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A repository on GitHub, named by host, owner, and name.
///
/// `gh pr list` asks one repository for its pull requests. This type holds the
/// three parts that name that repository. The host is in lowercase, because
/// host names ignore ASCII case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRepo {
    host: String,
    owner: String,
    name: String,
}

impl GithubRepo {
    /// Reads the URL of a git remote.
    ///
    /// Returns `Some` only when the host is `github.com` or equals
    /// `extra_host`. The caller passes the value of the `GH_HOST` environment
    /// variable as `extra_host`. An empty `extra_host` matches nothing.
    ///
    /// The function accepts the scp-like form `[user@]HOST:OWNER/NAME` and the
    /// URL forms `ssh://`, `https://`, `http://`, and `git://`. A trailing `/`
    /// and a trailing `.git` are optional. It returns `None` for local paths,
    /// `file://` URLs, other hosts, and paths that are not exactly
    /// `OWNER/NAME`. An SSH alias such as `github-work` is another host, so it
    /// returns `None` too.
    pub fn from_remote_url(url: &str, extra_host: Option<&str>) -> Option<Self> {
        let (host, path) = split_host_and_path(url)?;
        let host = host.to_ascii_lowercase();
        let wanted = host == GITHUB_HOST
            || extra_host
                .is_some_and(|extra| !extra.is_empty() && host == extra.to_ascii_lowercase());
        if !wanted {
            return None;
        }
        let path = path.strip_suffix('/').unwrap_or(path);
        let path = path.strip_suffix(GIT_SUFFIX).unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        if !is_name_part(owner) || !is_name_part(name) {
            return None;
        }
        Some(Self {
            host,
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    /// The owner of the repository, as it appears in the remote URL.
    pub fn owner(&self) -> &str {
        &self.owner
    }
}

impl fmt::Display for GithubRepo {
    /// Writes `host/owner/name`, the form that `gh pr list --repo` takes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.host, self.owner, self.name)
    }
}

/// The open pull requests of one repository, grouped by head branch.
///
/// Build it with `parse_pull_list`. Ask it for the numbers of one branch with
/// `numbers`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OpenPulls {
    by_branch: HashMap<String, Vec<PullNumber>>,
}

impl OpenPulls {
    /// The numbers of the open pull requests whose head is `branch`.
    ///
    /// The numbers are in ascending order and hold no duplicates. The slice is
    /// empty when the branch has no open pull request.
    pub fn numbers(&self, branch: &str) -> &[PullNumber] {
        self.by_branch.get(branch).map_or(&[], Vec::as_slice)
    }
}

/// One entry of the JSON array that `gh pr list` prints.
#[derive(Debug, Deserialize)]
struct RawPull {
    number: PullNumber,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "headRepositoryOwner", default)]
    head_repository_owner: Option<RawOwner>,
}

/// The owner of the repository that holds the head branch of a pull request.
#[derive(Debug, Deserialize)]
struct RawOwner {
    login: String,
}

/// Reads the output of `gh pr list --json number,headRefName,headRepositoryOwner`.
///
/// Keeps an entry only when the login of `headRepositoryOwner` equals `owner`,
/// with ASCII case ignored, because GitHub logins ignore case. This drops the
/// pull requests from forks, whose branch names can equal a local branch name.
/// An entry with a `null` or absent owner never matches. That is the case for
/// a pull request whose fork was deleted.
///
/// Returns the kept pull requests and the count of all entries in the array,
/// counted before the filter. The caller compares that count with the `--limit`
/// it gave to `gh`, to find out if the list is cut short.
///
/// # Errors
///
/// Returns the error of `serde_json` when `json` is not an array of objects of
/// the expected shape.
fn parse_pull_list(json: &str, owner: &str) -> Result<(OpenPulls, usize), serde_json::Error> {
    let raw: Vec<RawPull> = serde_json::from_str(json)?;
    let total = raw.len();
    let mut by_branch: HashMap<String, Vec<PullNumber>> = HashMap::new();
    for pull in raw {
        let from_owner = pull
            .head_repository_owner
            .is_some_and(|head| head.login.eq_ignore_ascii_case(owner));
        if from_owner {
            by_branch
                .entry(pull.head_ref_name)
                .or_default()
                .push(pull.number);
        }
    }
    for numbers in by_branch.values_mut() {
        numbers.sort_unstable();
        numbers.dedup();
    }
    Ok((OpenPulls { by_branch }, total))
}

/// The program that lists the pull requests.
const GH: &str = "gh";

/// The `--limit` that cwt gives to `gh pr list`.
///
/// The default of `gh pr list` is 30. A repository with more than 30 open pull
/// requests would lose numbers with no message.
pub const PULL_LIMIT: usize = 1000;

/// The JSON fields that cwt asks `gh pr list` for.
const GH_JSON_FIELDS: &str = "number,headRefName,headRepositoryOwner";

/// Why `run_with_limit` gave no output.
#[derive(Debug)]
enum RunFailure {
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
fn run_with_limit(command: &mut Command, limit: Duration) -> Result<Output, RunFailure> {
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

/// What the lookup found for one repository.
///
/// `Lookup::default()` means there is nothing to show and nothing to say. That
/// is the normal state for a repository with no GitHub remote, and for a
/// machine with no `gh`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Lookup {
    pulls: OpenPulls,
    warning: Option<String>,
}

impl Lookup {
    /// The open pull requests that the lookup found.
    pub fn pulls(&self) -> &OpenPulls {
        &self.pulls
    }

    /// One line that tells the user what went wrong, without a `Warning: `
    /// prefix. The caller adds the prefix. `None` means nothing went wrong.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// A lookup that found no pull requests and carries `warning`.
    pub fn failed(warning: String) -> Self {
        Self {
            pulls: OpenPulls::default(),
            warning: Some(warning),
        }
    }
}

/// Lists the open pull requests of the repository in `repo_dir`.
///
/// Reads `GH_HOST` and calls `lookup_with` with the real `gh`. See
/// `lookup_with` for the rules.
pub fn lookup(repo_dir: &Path, limit: Duration) -> Lookup {
    let gh_host = std::env::var("GH_HOST").ok();
    lookup_with(OsStr::new(GH), repo_dir, limit, gh_host.as_deref())
}

/// Lists the open pull requests of the repository in `repo_dir` with the
/// program `gh`.
///
/// The program is a parameter so a test can pass a stub script and never change
/// the process-global `PATH`.
fn lookup_with(
    gh: &OsStr,
    repo_dir: &Path,
    limit: Duration,
    extra_host: Option<&str>,
) -> Lookup {
    let _ = (gh, repo_dir, limit, extra_host);
    Lookup::failed("not implemented".to_owned())
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
        assert!(matches!(result, Err(RunFailure::NotInstalled)), "{result:?}");
    }

    #[test]
    fn run_with_limit_reports_a_program_that_cannot_start() {
        let dir = TempDir::new().unwrap();
        let not_executable = dir.path().join("plain");
        std::fs::write(&not_executable, "text").unwrap();
        let result = run_with_limit(&mut Command::new(&not_executable), GENEROUS_LIMIT);
        assert!(matches!(result, Err(RunFailure::Spawn(_))), "{result:?}");
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

    #[test]
    fn pull_number_displays_with_a_hash() {
        assert_eq!(PullNumber(547).to_string(), "#547");
    }

    #[test]
    fn pull_number_reads_from_a_bare_json_number() {
        let number: PullNumber = serde_json::from_str("548").unwrap();
        assert_eq!(number, PullNumber(548));
    }

    /// Reads `url` with no extra host and gives the `host/owner/name` form.
    fn parsed(url: &str) -> Option<String> {
        GithubRepo::from_remote_url(url, None).map(|repo| repo.to_string())
    }

    #[test]
    fn scp_like_form_is_read_with_and_without_dot_git() {
        assert_eq!(
            parsed("git@github.com:owner/name.git").as_deref(),
            Some("github.com/owner/name")
        );
        assert_eq!(
            parsed("git@github.com:owner/name").as_deref(),
            Some("github.com/owner/name")
        );
        assert_eq!(
            parsed("github.com:owner/name").as_deref(),
            Some("github.com/owner/name")
        );
    }

    #[test]
    fn https_form_is_read_in_every_spelling() {
        for url in [
            "https://github.com/owner/name",
            "https://github.com/owner/name.git",
            "https://github.com/owner/name/",
            "https://github.com/owner/name.git/",
            "https://user@github.com/owner/name",
            "https://github.com:443/owner/name",
            "http://github.com/owner/name",
        ] {
            assert_eq!(
                parsed(url).as_deref(),
                Some("github.com/owner/name"),
                "{url}"
            );
        }
    }

    #[test]
    fn ssh_and_git_schemes_are_read() {
        for url in [
            "ssh://git@github.com/owner/name.git",
            "ssh://git@github.com:22/owner/name.git",
            "git://github.com/owner/name.git",
            "HTTPS://github.com/owner/name",
        ] {
            assert_eq!(
                parsed(url).as_deref(),
                Some("github.com/owner/name"),
                "{url}"
            );
        }
    }

    #[test]
    fn host_comparison_ignores_case_and_stores_lowercase() {
        assert_eq!(
            parsed("git@GitHub.com:owner/name.git").as_deref(),
            Some("github.com/owner/name")
        );
    }

    #[test]
    fn owner_is_the_owner_part() {
        let repo = GithubRepo::from_remote_url("git@github.com:Owner/name.git", None).unwrap();
        assert_eq!(repo.owner(), "Owner");
    }

    #[test]
    fn extra_host_is_matched_only_when_given_and_not_empty() {
        let url = "git@ghe.example.com:org/repo.git";
        let repo = GithubRepo::from_remote_url(url, Some("ghe.example.com")).unwrap();
        assert_eq!(repo.to_string(), "ghe.example.com/org/repo");
        assert!(GithubRepo::from_remote_url(url, Some("GHE.Example.com")).is_some());
        assert!(GithubRepo::from_remote_url(url, None).is_none());
        assert!(GithubRepo::from_remote_url(url, Some("")).is_none());
        assert!(GithubRepo::from_remote_url("git@:org/repo.git", Some("")).is_none());
        assert!(GithubRepo::from_remote_url(url, Some("other.example.com")).is_none());
    }

    #[test]
    fn urls_that_are_not_a_github_repository_are_refused() {
        for url in [
            "git@gitlab.com:owner/name.git",
            "https://gitlab.com/owner/name",
            "/srv/repo.git",
            "../repo",
            "file:///srv/x.git",
            "git@github-work:owner/name.git",
            "https://github.com/a/b/c",
            "https://github.com/owner",
            "https://github.com/owner/",
            "https://github.com//name",
            "https://github.com/owner/.git",
            "https://github.com/owner/na me",
            "https://github.com/owner/na\u{e9}me",
            "https://github.com/\u{65e5}\u{672c}/name",
            "",
        ] {
            assert_eq!(parsed(url), None, "{url}");
        }
    }

    /// One entry in the shape that `gh pr list --json` prints.
    fn entry(number: u64, branch: &str, login: &str) -> String {
        format!(
            r#"{{"headRefName":"{branch}","headRepositoryOwner":{{"id":"MDQ6","name":"Some One","login":"{login}"}},"number":{number}}}"#
        )
    }

    /// Joins entries into a JSON array.
    fn array(entries: &[String]) -> String {
        format!("[{}]", entries.join(","))
    }

    fn numbers_of(pulls: &OpenPulls, branch: &str) -> Vec<u64> {
        pulls.numbers(branch).iter().map(|n| n.0).collect()
    }

    #[test]
    fn one_pull_request_is_found_by_its_branch() {
        let json = array(&[entry(548, "issue-546", "timmattison")]);
        let (pulls, total) = parse_pull_list(&json, "timmattison").unwrap();
        assert_eq!(numbers_of(&pulls, "issue-546"), vec![548]);
        assert_eq!(total, 1);
    }

    #[test]
    fn numbers_of_one_branch_are_sorted_and_unique() {
        let json = array(&[
            entry(30, "feat", "me"),
            entry(7, "feat", "me"),
            entry(19, "feat", "me"),
            entry(7, "feat", "me"),
        ]);
        let (pulls, _) = parse_pull_list(&json, "me").unwrap();
        assert_eq!(numbers_of(&pulls, "feat"), vec![7, 19, 30]);
    }

    #[test]
    fn a_branch_without_a_pull_request_has_an_empty_slice() {
        let json = array(&[entry(1, "feat", "me")]);
        let (pulls, _) = parse_pull_list(&json, "me").unwrap();
        assert!(pulls.numbers("other").is_empty());
        assert!(OpenPulls::default().numbers("feat").is_empty());
    }

    #[test]
    fn pull_requests_from_other_owners_are_not_kept() {
        let json = array(&[
            entry(1, "main", "stranger"),
            entry(2, "patch-1", "stranger"),
            entry(3, "main", "me"),
        ]);
        let (pulls, _) = parse_pull_list(&json, "me").unwrap();
        assert_eq!(numbers_of(&pulls, "main"), vec![3]);
        assert!(pulls.numbers("patch-1").is_empty());
    }

    #[test]
    fn owner_comparison_ignores_ascii_case() {
        let json = array(&[entry(5, "feat", "TimMattison")]);
        let (pulls, _) = parse_pull_list(&json, "timmattison").unwrap();
        assert_eq!(numbers_of(&pulls, "feat"), vec![5]);
    }

    #[test]
    fn null_and_absent_owners_are_not_kept() {
        let json = r#"[
            {"headRefName":"a","headRepositoryOwner":null,"number":1},
            {"headRefName":"a","number":2},
            {"headRefName":"a","headRepositoryOwner":{"login":"me"},"number":3}
        ]"#;
        let (pulls, total) = parse_pull_list(json, "me").unwrap();
        assert_eq!(numbers_of(&pulls, "a"), vec![3]);
        assert_eq!(total, 3);
    }

    #[test]
    fn the_total_counts_entries_the_filter_dropped() {
        let json = array(&[
            entry(1, "a", "me"),
            entry(2, "b", "stranger"),
            entry(3, "c", "other"),
        ]);
        let (_, total) = parse_pull_list(&json, "me").unwrap();
        assert_eq!(total, 3);
        let (pulls, total) = parse_pull_list("[]", "me").unwrap();
        assert_eq!((pulls, total), (OpenPulls::default(), 0));
    }

    #[test]
    fn multibyte_branch_names_round_trip() {
        let json = array(&[
            entry(1, "\u{65e5}\u{672c}\u{8a9e}", "me"),
            entry(2, "\u{1f389}", "me"),
            entry(3, "caf\u{e9}", "me"),
        ]);
        let (pulls, _) = parse_pull_list(&json, "me").unwrap();
        assert_eq!(numbers_of(&pulls, "\u{65e5}\u{672c}\u{8a9e}"), vec![1]);
        assert_eq!(numbers_of(&pulls, "\u{1f389}"), vec![2]);
        assert_eq!(numbers_of(&pulls, "caf\u{e9}"), vec![3]);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_pull_list("not json", "me").is_err());
        assert!(parse_pull_list(r#"{"number":1}"#, "me").is_err());
        assert!(parse_pull_list(r#"[{"number":1}]"#, "me").is_err());
    }

    /// Runs a git command in `dir` with a hermetic environment and checks that
    /// it worked.
    fn git(dir: &Path, args: &[&str]) {
        let mut command = Command::new("git");
        gitscratch::shed_inherited_git_environment(&mut command);
        let output = command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }

    /// A repository in a fresh temporary directory. `origin` is its remote URL,
    /// or `None` for a repository with no remote.
    fn repo_with_origin(origin: Option<&str>) -> TempDir {
        let dir = TempDir::new().unwrap();
        git(dir.path(), &["init", "-q"]);
        if let Some(url) = origin {
            git(dir.path(), &["remote", "add", "origin", url]);
        }
        dir
    }

    /// The file in `dir` that a stub writes its record to.
    fn record(dir: &TempDir, name: &str) -> std::path::PathBuf {
        dir.path().join(name)
    }

    /// The JSON that `gh pr list` prints for `entries`, as `(number, branch,
    /// owner)`.
    fn gh_json(entries: &[(u64, &str, &str)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(number, branch, owner)| entry(*number, branch, owner))
            .collect();
        array(&items)
    }

    /// A stub `gh` that records its arguments and prints `json`.
    fn answering_stub(dir: &TempDir, json: &str) -> std::path::PathBuf {
        let answer = record(dir, "answer.json");
        std::fs::write(&answer, json).unwrap();
        write_stub(
            dir,
            "gh-stub",
            &format!(
                "printf '%s\\n' \"$@\" > \"{args}\"\n: > \"{ran}\"\nenv > \"{env}\"\ncat \"{answer}\"",
                args = record(dir, "args").display(),
                ran = record(dir, "ran").display(),
                env = record(dir, "env").display(),
                answer = answer.display(),
            ),
        )
    }

    fn run_lookup(stub: &Path, repo: &TempDir, limit: Duration) -> Lookup {
        lookup_with(stub.as_os_str(), repo.path(), limit, None)
    }

    #[test]
    fn lookup_finds_the_numbers_of_the_owner() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@github.com:owner/one.git"));
        let json = gh_json(&[
            (7, "feat", "owner"),
            (9, "feat", "owner"),
            (8, "feat", "forker"),
            (3, "other", "forker"),
        ]);
        let stub = answering_stub(&work, &json);
        let found = run_lookup(&stub, &repo, GENEROUS_LIMIT);
        assert_eq!(found.warning(), None);
        let numbers: Vec<String> = found
            .pulls()
            .numbers("feat")
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(numbers, ["#7", "#9"]);
        assert!(found.pulls().numbers("other").is_empty());
    }

    #[test]
    fn lookup_gives_gh_the_repository_and_the_limit() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@github.com:owner/one.git"));
        let stub = answering_stub(&work, "[]");
        run_lookup(&stub, &repo, GENEROUS_LIMIT);
        let args = std::fs::read_to_string(record(&work, "args")).unwrap();
        let args: Vec<&str> = args.lines().collect();
        assert_eq!(
            args,
            [
                "pr",
                "list",
                "--repo",
                "github.com/owner/one",
                "--state",
                "open",
                "--limit",
                "1000",
                "--json",
                "number,headRefName,headRepositoryOwner",
            ]
        );
    }

    #[test]
    fn lookup_asks_gh_for_plain_json() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@github.com:owner/one.git"));
        let stub = answering_stub(&work, "[]");
        run_lookup(&stub, &repo, GENEROUS_LIMIT);
        let env = std::fs::read_to_string(record(&work, "env")).unwrap();
        assert!(env.lines().any(|line| line == "NO_COLOR=1"), "{env}");
    }

    #[test]
    fn lookup_is_silent_for_a_repository_with_no_origin() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(None);
        let stub = answering_stub(&work, "[]");
        assert_eq!(run_lookup(&stub, &repo, GENEROUS_LIMIT), Lookup::default());
        assert!(!record(&work, "ran").exists());
    }

    #[test]
    fn lookup_is_silent_for_a_remote_that_is_not_github() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@gitlab.com:owner/one.git"));
        let stub = answering_stub(&work, "[]");
        assert_eq!(run_lookup(&stub, &repo, GENEROUS_LIMIT), Lookup::default());
        assert!(!record(&work, "ran").exists());
    }

    #[test]
    fn lookup_is_silent_when_gh_is_not_installed() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@github.com:owner/one.git"));
        let missing = record(&work, "no-such-gh");
        assert_eq!(
            run_lookup(&missing, &repo, GENEROUS_LIMIT),
            Lookup::default()
        );
    }

    #[test]
    fn lookup_uses_the_extra_host_for_a_github_enterprise_remote() {
        let work = TempDir::new().unwrap();
        let repo = repo_with_origin(Some("git@ghe.example.com:org/repo.git"));
        let stub = answering_stub(&work, "[]");
        let found = lookup_with(
            stub.as_os_str(),
            repo.path(),
            GENEROUS_LIMIT,
            Some("ghe.example.com"),
        );
        assert_eq!(found.warning(), None);
        let args = std::fs::read_to_string(record(&work, "args")).unwrap();
        assert!(args.lines().any(|line| line == "ghe.example.com/org/repo"));
    }
}
