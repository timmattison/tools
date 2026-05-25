//! crap — "Claude, Resume Anywhere Please".
//!
//! Resume a Claude Code session from whatever directory it was originally
//! started in, no matter where you are now. Given a session id, `crap` looks
//! up that session under `~/.claude/projects`, recovers the directory it ran
//! in, changes into it, and re-launches Claude with `--resume <id>`.
//!
//! With `--here`, it instead brings the session to *you*: Claude resolves a
//! `--resume <id>` only against the project folder matching the current working
//! directory, so `crap --here` symlinks the session's transcript into that
//! folder and resumes it as a `--fork-session` (a fresh id), leaving the
//! original transcript untouched. The symlink is removed once the session ends.
//!
//! Because a binary cannot change its parent shell's working directory (nor see
//! shell aliases such as `clauded`), the user-facing `crap` command is a shell
//! function installed via `crap --shell-setup`. This binary resolves the session
//! id — printing the original directory to resume from, or (for `--here`)
//! preparing the symlink and printing what the function should run and clean up.

use std::path::{Path, PathBuf};
use std::process::exit;

use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use shellsetup::ShellIntegration;

/// Exit codes for the different failure conditions.
mod exit_codes {
    /// No session file matched the given id.
    pub const SESSION_NOT_FOUND: i32 = 1;
    /// The session file had no recorded working directory.
    pub const NO_CWD_IN_SESSION: i32 = 2;
    /// The recorded working directory no longer exists.
    pub const DIRECTORY_MISSING: i32 = 3;
    /// The session id was not a valid filename component.
    pub const INVALID_SESSION_ID: i32 = 4;
    /// Shell setup failed.
    pub const SHELL_SETUP_ERROR: i32 = 5;
    /// The user's home directory could not be determined.
    pub const NO_HOME_DIR: i32 = 6;
    /// The session is already running in another process.
    pub const SESSION_ALREADY_RUNNING: i32 = 7;
    /// `--here`: the project folder or symlink could not be created.
    pub const HERE_LINK_ERROR: i32 = 8;
    /// `--here`: the current working directory could not be determined.
    pub const HERE_PWD_UNAVAILABLE: i32 = 9;
}

/// Why a session id could not be resolved to an existing directory.
#[derive(Debug)]
enum ResolveError {
    /// The session id contained path separators or traversal sequences.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any project directory.
    SessionNotFound,
    /// The session file exists but records no working directory.
    NoCwdInSession,
    /// The recorded working directory no longer exists on disk.
    DirectoryMissing(PathBuf),
}

/// Returns `true` if `id` is a canonical UUID (`8-4-4-4-12` hex digits).
///
/// Claude session ids are always UUIDs and the id only ever names a `.jsonl`
/// file under `~/.claude/projects`. Requiring this exact shape rejects typo'd
/// ids up front and, as a side effect, guarantees no path separator, traversal
/// sequence, or shell metacharacter can ride through to the filesystem lookup
/// or the shell function. Hex is matched case-insensitively.
fn is_valid_session_id(id: &str) -> bool {
    /// Hyphen positions in a canonical UUID, and its total length.
    const HYPHEN_POSITIONS: [usize; 4] = [8, 13, 18, 23];
    const UUID_LEN: usize = 36;

    if id.len() != UUID_LEN {
        return false;
    }
    id.bytes().enumerate().all(|(i, b)| {
        if HYPHEN_POSITIONS.contains(&i) {
            b == b'-'
        } else {
            b.is_ascii_hexdigit()
        }
    })
}

/// Extracts the first non-empty `cwd` value from Claude session JSONL contents.
///
/// Each line is an independent JSON object; the early bookkeeping lines often
/// carry `"cwd": null`, so the first line with a non-empty string `cwd` wins.
/// Non-JSON lines are skipped. Returns `None` if no usable `cwd` is present.
fn extract_cwd(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
    }
    None
}

/// Locates `<session_id>.jsonl` inside any immediate subdirectory of
/// `projects_dir`, returning its full path if found.
fn find_session_file(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    let file_name = format!("{session_id}.jsonl");
    for entry in std::fs::read_dir(projects_dir).ok()?.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let candidate = entry.path().join(&file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves a session id to the existing directory the session ran in.
///
/// # Errors
///
/// Returns a [`ResolveError`] when the id is invalid, no session file matches,
/// the session records no working directory, or that directory no longer
/// exists.
fn resolve_session_dir(projects_dir: &Path, session_id: &str) -> Result<PathBuf, ResolveError> {
    if !is_valid_session_id(session_id) {
        return Err(ResolveError::InvalidSessionId);
    }
    let file = find_session_file(projects_dir, session_id).ok_or(ResolveError::SessionNotFound)?;
    let contents = std::fs::read_to_string(&file).map_err(|_| ResolveError::SessionNotFound)?;
    let cwd = extract_cwd(&contents).ok_or(ResolveError::NoCwdInSession)?;
    let path = PathBuf::from(cwd);
    if !path.is_dir() {
        return Err(ResolveError::DirectoryMissing(path));
    }
    Ok(path)
}

/// Encodes a working directory into the project-folder name Claude Code uses
/// under `~/.claude/projects`.
///
/// Claude derives the folder name by replacing every character of the absolute
/// path that is not an ASCII letter or digit with `-`. So `/` and `.` both
/// become `-` (and a `/.` run becomes `--`), while existing hyphens are kept.
/// This is the lookup `claude --resume` performs against the *current*
/// directory, so reproducing it exactly is what lets `--here` drop a session
/// where Claude will find it.
///
/// The mapping is per Unicode scalar, which matches Claude for the ASCII paths
/// that real project directories use; non-ASCII characters each collapse to a
/// single `-`.
fn encode_project_dir(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Returns the `~/.claude/projects` directory, or `None` if the home directory
/// cannot be determined.
fn claude_projects_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("projects"))
}

/// Makes the session `original_jsonl` resolvable by `claude --resume` from
/// `pwd`, by symlinking it into `pwd`'s project folder under `projects_dir`.
///
/// Returns the path of the symlink that was created (so the caller can remove it
/// once the session ends), or `None` when the session is *already* resolvable
/// from `pwd` — because `pwd` is its own directory, or an earlier `--here`
/// already linked it. Anything already at the target name is left untouched: a
/// session id is a UUID, so a file there can only be this very session, and
/// clobbering it would never be correct.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the project folder or the
/// symlink cannot be created.
fn prepare_here_link(
    projects_dir: &Path,
    original_jsonl: &Path,
    pwd: &Path,
    session_id: &str,
) -> std::io::Result<Option<PathBuf>> {
    let folder = projects_dir.join(encode_project_dir(pwd));
    let link_path = folder.join(format!("{session_id}.jsonl"));

    // Anything already at this name means the session resolves from here
    // already. `symlink_metadata` does not follow links, so even a dangling
    // symlink left by an earlier `--here` counts as "present".
    if link_path.symlink_metadata().is_ok() {
        return Ok(None);
    }

    std::fs::create_dir_all(&folder)?;

    #[cfg(unix)]
    std::os::unix::fs::symlink(original_jsonl, &link_path)?;
    #[cfg(not(unix))]
    std::os::windows::fs::symlink_file(original_jsonl, &link_path)?;

    Ok(Some(link_path))
}

/// Why `--here` could not place a session under the current directory.
#[derive(Debug)]
enum HereResolveError {
    /// The session id was not a valid UUID.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any project directory.
    SessionNotFound,
    /// Creating the project folder or the symlink failed.
    Io(std::io::Error),
}

/// Validates `session_id`, locates its transcript, and symlinks it into `pwd`'s
/// project folder so `claude --resume` will find it from there.
///
/// Returns the path of the symlink to clean up afterwards, or `None` when the
/// session is already resolvable from `pwd` (no symlink needed).
///
/// # Errors
///
/// See [`HereResolveError`].
fn resolve_here_link(
    projects_dir: &Path,
    pwd: &Path,
    session_id: &str,
) -> Result<Option<PathBuf>, HereResolveError> {
    if !is_valid_session_id(session_id) {
        return Err(HereResolveError::InvalidSessionId);
    }
    let original = find_session_file(projects_dir, session_id)
        .ok_or(HereResolveError::SessionNotFound)?;
    prepare_here_link(projects_dir, &original, pwd, session_id).map_err(HereResolveError::Io)
}

/// Returns the `~/.claude/sessions` directory, or `None` if the home directory
/// cannot be determined.
///
/// Claude Code writes one `<pid>.json` file here per live CLI session and
/// removes it on clean exit, so it serves as a registry of running sessions.
fn claude_sessions_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("sessions"))
}

/// A running Claude CLI session, as recorded under `~/.claude/sessions`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionRecord {
    /// The process id of the running `claude` process.
    pid: u32,
    /// The session id this process is attached to.
    session_id: String,
    /// The directory the session is running in.
    cwd: String,
    /// The reported activity status (e.g. `"busy"` or `"idle"`), if present.
    status: Option<String>,
}

/// Parses a `~/.claude/sessions/<pid>.json` record.
///
/// Returns `None` if the JSON is malformed or is missing the `pid`/`sessionId`
/// fields that uniquely identify a running session.
fn parse_session_record(json: &str) -> Option<SessionRecord> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let session_id = value.get("sessionId")?.as_str()?.to_string();
    let cwd = value
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(SessionRecord {
        pid,
        session_id,
        cwd,
        status,
    })
}

/// Finds a currently-running session attached to `session_id`.
///
/// Scans every `<pid>.json` under `sessions_dir` and returns the first record
/// whose `session_id` matches and whose pid `is_alive` reports as still
/// running. The `is_alive` predicate is injected so this logic is testable
/// without spawning real processes.
fn find_live_session<F>(sessions_dir: &Path, session_id: &str, is_alive: F) -> Option<SessionRecord>
where
    F: Fn(u32) -> bool,
{
    for entry in std::fs::read_dir(sessions_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(record) = parse_session_record(&contents) else {
            continue;
        };
        if record.session_id == session_id && is_alive(record.pid) {
            return Some(record);
        }
    }
    None
}

/// Reports whether `pid` is a currently-running Claude CLI process.
///
/// Uses `ps -p <pid> -o command=`: a non-empty, successful result means the pid
/// exists, and requiring `claude` in the command line guards against a stale
/// session file whose pid has since been reused by an unrelated process.
fn pid_is_alive(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .to_lowercase()
        .contains("claude")
}

/// Formats the binary's success output for the shell function to read back.
///
/// The session id (a validated UUID) is emitted first, on its own line; the
/// directory comes last. Putting the variable-content directory last means a
/// path that itself contains a newline can't be mistaken for the end of the
/// session-id field — the shell function takes the first line as the session id
/// and everything after it as the directory.
fn format_output(dir: &Path, session_id: &str) -> String {
    format!("{session_id}\n{}\n", dir.display())
}

/// Leading token marking `--here` output, distinguishing it from the default
/// `<session-id>\n<dir>` resume output the shell function otherwise expects.
const HERE_SENTINEL: &str = "__CRAP_HERE__";

/// Placeholder used in the link field when `--here` created no symlink (because
/// the current directory already is the session's own folder), so the shell
/// function can tell "nothing to clean up" apart from a real path.
const NO_LINK_SENTINEL: &str = "__CRAP_NO_LINK__";

/// Formats `--here` output for the shell function: the [`HERE_SENTINEL`], then
/// the session id, then the symlink to remove once the session ends (or
/// [`NO_LINK_SENTINEL`] when none was created).
///
/// The cleanup path is emitted last so that — like [`format_output`] — a path
/// containing a newline survives intact as "everything after the second line".
fn format_here_output(session_id: &str, link_to_cleanup: Option<&Path>) -> String {
    let link = match link_to_cleanup {
        Some(path) => path.display().to_string(),
        None => NO_LINK_SENTINEL.to_string(),
    };
    format!("{HERE_SENTINEL}\n{session_id}\n{link}\n")
}

/// The shell function installed by `crap --shell-setup`.
///
/// `crap` shadows the binary, so the function reaches the binary explicitly via
/// `command crap`, forwarding all arguments (so flags like `--force` and
/// `--here` work). `clauded` is resolved through `eval` so that an alias of that
/// name is expanded at call time (shell aliases are otherwise not expanded
/// inside function bodies); if no `clauded` exists, plain `claude` is used. If
/// the binary exits non-zero (session not found, already running, …) its message
/// is shown and the function does nothing further.
///
/// The binary speaks one of two output shapes:
///
/// * **default** — `<session-id>\n<dir>`: the function `cd`s into the original
///   directory (splitting on the first newline so a path containing newlines
///   survives intact) and resumes.
/// * **`--here`** — `__CRAP_HERE__\n<session-id>\n<link-or-sentinel>`: the binary
///   has already symlinked the session into the *current* directory's project
///   folder, so the function stays put, resumes with `--fork-session` (a fresh
///   session id, leaving the original transcript untouched), and finally removes
///   that symlink — unless the link field is `__CRAP_NO_LINK__`, meaning none
///   was created because this already is the session's own directory.
const SHELL_CODE: &str = r#"
function crap() {
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_HERE__" ]; then
        local __crap_rest __crap_session __crap_link
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_link=${__crap_rest#*$'\n'}
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded --resume "$__crap_session" --fork-session'
        else
            claude --resume "$__crap_session" --fork-session
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            rm -f -- "$__crap_link"
        fi
        return
    fi
    local __crap_session __crap_dir
    __crap_session=${__crap_out%%$'\n'*}
    __crap_dir=${__crap_out#*$'\n'}
    cd -- "$__crap_dir" || return 1
    if command -v clauded >/dev/null 2>&1; then
        eval 'clauded --resume "$__crap_session"'
    else
        claude --resume "$__crap_session"
    fi
}
"#;

/// Installs the `crap` shell function into the user's shell config.
fn setup_shell_integration() -> Result<(), shellsetup::ShellSetupError> {
    let integration = ShellIntegration::new(
        "crap",
        "Claude, Resume Anywhere Please",
        SHELL_CODE,
    )
    .with_command("crap", "Resume a Claude session from its original directory");
    integration.setup()
}

/// Command-line interface for `crap`.
#[derive(Parser)]
#[command(name = "crap")]
#[command(
    about = "Claude, Resume Anywhere Please — resume a Claude session from its original directory"
)]
#[command(version = version_string!())]
struct Cli {
    /// The Claude session id to resume (the `.jsonl` filename under
    /// `~/.claude/projects`).
    #[arg(value_name = "SESSION_ID", required_unless_present = "shell_setup")]
    session_id: Option<String>,

    /// Resume even if the session appears to be running in another process.
    ///
    /// By default `crap` refuses to resume a session that is already open
    /// elsewhere, because two processes writing the same session log can
    /// corrupt it.
    #[arg(short, long)]
    force: bool,

    /// Resume the session in the *current* directory instead of its original.
    ///
    /// `crap` symlinks the session into the current directory's project folder
    /// so `claude --resume` can find it here, then resumes it as a forked
    /// (new-id) session — the original transcript is left untouched. Use this to
    /// carry a conversation's context into a different working directory.
    #[arg(long)]
    here: bool,

    /// Install the `crap` shell function into your shell config, then exit.
    ///
    /// Run this once: `crap --shell-setup`. After re-sourcing your shell,
    /// `crap <session-id>` will cd into the session's directory and resume it.
    #[arg(long)]
    shell_setup: bool,
}

/// Aborts with a clear message if `session_id` is already open in another live
/// process, unless `force` overrides the check.
///
/// Both resume modes call this: two processes writing one session log can
/// corrupt it, and even `--here`'s fork reads the live transcript while it is
/// being written. `--force` bypasses the guard.
fn abort_if_session_live(session_id: &str, force: bool) {
    if force {
        return;
    }
    if let Some(live) =
        claude_sessions_dir().and_then(|s| find_live_session(&s, session_id, pid_is_alive))
    {
        let status = live.status.as_deref().unwrap_or("running");
        eprintln!(
            "{} session '{session_id}' is already running (pid {}, {status})",
            "Error:".red().bold(),
            live.pid
        );
        eprintln!("       in {}", live.cwd);
        eprintln!("       resuming it again can corrupt the session log.");
        eprintln!(
            "       re-run with {} to resume anyway.",
            "--force".yellow()
        );
        exit(exit_codes::SESSION_ALREADY_RUNNING);
    }
}

/// Handles `crap --here <id>`: symlink the session into the current directory's
/// project folder and emit the here-mode output the shell function consumes.
fn run_here(projects_dir: &Path, session_id: &str, force: bool) -> ! {
    let Ok(pwd) = std::env::current_dir() else {
        eprintln!(
            "{} could not determine the current directory",
            "Error:".red().bold()
        );
        exit(exit_codes::HERE_PWD_UNAVAILABLE);
    };

    // Guard before creating anything, so an aborted resume leaves no stray link.
    abort_if_session_live(session_id, force);

    match resolve_here_link(projects_dir, &pwd, session_id) {
        Ok(link) => {
            print!("{}", format_here_output(session_id, link.as_deref()));
            exit(0);
        }
        Err(HereResolveError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(HereResolveError::SessionNotFound) => {
            eprintln!(
                "{} no Claude session found with id '{session_id}'",
                "Error:".red().bold()
            );
            eprintln!("       looked under {}", projects_dir.display());
            exit(exit_codes::SESSION_NOT_FOUND);
        }
        Err(HereResolveError::Io(err)) => {
            eprintln!(
                "{} could not prepare this directory for '{session_id}': {err}",
                "Error:".red().bold()
            );
            exit(exit_codes::HERE_LINK_ERROR);
        }
    }
}

fn main() {
    let cli = Cli::parse();

    if cli.shell_setup {
        match setup_shell_integration() {
            Ok(()) => exit(0),
            Err(e) => {
                eprintln!("{} {e}", "Error:".red().bold());
                exit(exit_codes::SHELL_SETUP_ERROR);
            }
        }
    }

    // `required_unless_present = "shell_setup"` guarantees this is present here.
    let session_id = cli.session_id.expect("session id is required without --shell-setup");

    let Some(projects_dir) = claude_projects_dir() else {
        eprintln!(
            "{} could not determine your home directory",
            "Error:".red().bold()
        );
        exit(exit_codes::NO_HOME_DIR);
    };

    if cli.here {
        run_here(&projects_dir, &session_id, cli.force);
    }

    match resolve_session_dir(&projects_dir, &session_id) {
        Ok(dir) => {
            abort_if_session_live(&session_id, cli.force);
            print!("{}", format_output(&dir, &session_id));
        }
        Err(ResolveError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(ResolveError::SessionNotFound) => {
            eprintln!(
                "{} no Claude session found with id '{session_id}'",
                "Error:".red().bold()
            );
            eprintln!("       looked under {}", projects_dir.display());
            exit(exit_codes::SESSION_NOT_FOUND);
        }
        Err(ResolveError::NoCwdInSession) => {
            eprintln!(
                "{} session '{session_id}' has no recorded working directory",
                "Error:".red().bold()
            );
            exit(exit_codes::NO_CWD_IN_SESSION);
        }
        Err(ResolveError::DirectoryMissing(path)) => {
            eprintln!(
                "{} the directory for session '{session_id}' no longer exists:",
                "Error:".red().bold()
            );
            eprintln!("       {}", path.display());
            exit(exit_codes::DIRECTORY_MISSING);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// A representative session id used across tests.
    const SAMPLE_ID: &str = "11111111-2222-3333-4444-555555555555";

    /// Builds a single JSONL line recording `cwd`.
    fn cwd_line(cwd: &str) -> String {
        format!("{}\n", serde_json::json!({ "cwd": cwd }))
    }

    #[test]
    fn extract_cwd_returns_first_non_null_cwd() {
        let contents = format!(
            "{}{}{}{}",
            "{\"type\":\"summary\"}\n",
            "{\"cwd\":null}\n",
            cwd_line("/Users/tim/code/foo"),
            cwd_line("/Users/tim/code/bar"),
        );
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/code/foo")
        );
    }

    #[test]
    fn extract_cwd_skips_non_json_and_missing_or_empty() {
        let contents = "not json at all\n{\"foo\":1}\n{\"cwd\":\"\"}\n";
        assert_eq!(extract_cwd(contents), None);
    }

    #[test]
    fn extract_cwd_empty_input_is_none() {
        assert_eq!(extract_cwd(""), None);
    }

    #[test]
    fn extract_cwd_handles_multibyte_paths() {
        let contents = cwd_line("/Users/tim/コード/café");
        assert_eq!(
            extract_cwd(&contents).as_deref(),
            Some("/Users/tim/コード/café")
        );
    }

    #[test]
    fn valid_session_id_accepts_uuid() {
        assert!(is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e1901"));
    }

    #[test]
    fn valid_session_id_rejects_traversal_and_separators() {
        assert!(!is_valid_session_id(""));
        assert!(!is_valid_session_id(".."));
        assert!(!is_valid_session_id("../etc/passwd"));
        assert!(!is_valid_session_id("a/b"));
        assert!(!is_valid_session_id("a\\b"));
    }

    #[test]
    fn valid_session_id_requires_uuid_shape() {
        // Anything that isn't a canonical 8-4-4-4-12 UUID is rejected, so a
        // typo'd id fails fast and shell metacharacters never reach the binary.
        assert!(!is_valid_session_id("not-a-uuid"));
        assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e190")); // too short
        assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e19011")); // too long
        assert!(!is_valid_session_id("4733ee2a1ad64619a01a11840b8e1901")); // no hyphens
        assert!(!is_valid_session_id("4733ee2g-1ad6-4619-a01a-11840b8e1901")); // 'g' not hex
        assert!(!is_valid_session_id("4733ee2a-1ad6-4619-a01a-11840b8e1901 ; rm -rf ~"));
        assert!(!is_valid_session_id("4733ee2a 1ad6 4619 a01a 11840b8e1901")); // spaces
    }

    #[test]
    fn valid_session_id_accepts_uppercase_hex() {
        // Hex is case-insensitive even though Claude writes lowercase ids.
        assert!(is_valid_session_id("4733EE2A-1AD6-4619-A01A-11840B8E1901"));
    }

    #[test]
    fn encode_project_dir_matches_claude_folder_naming() {
        // Plain path: every separator becomes a dash, the leading slash too.
        assert_eq!(
            encode_project_dir(Path::new("/Volumes/SamsungSSDs/code/claude-vibecoding")),
            "-Volumes-SamsungSSDs-code-claude-vibecoding"
        );
        // A `/.` run (hidden directory) collapses to a double dash, and the
        // hyphen already in the final component is preserved verbatim. This is
        // a real example observed under ~/.claude/projects.
        assert_eq!(
            encode_project_dir(Path::new("/Users/timmattison/.config/qbittorrent-vpn")),
            "-Users-timmattison--config-qbittorrent-vpn"
        );
        // Digits survive; only non-alphanumerics are rewritten.
        assert_eq!(
            encode_project_dir(Path::new("/a/day-3-planning")),
            "-a-day-3-planning"
        );
        // Non-ASCII characters each collapse to a single dash.
        assert_eq!(encode_project_dir(Path::new("/x/café")), "-x-caf-");
    }

    #[test]
    fn prepare_here_link_creates_symlink_in_pwd_project_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        // A pwd whose encoded project folder does not exist yet.
        let pwd = Path::new("/Volumes/x/here-cwd");
        let link = prepare_here_link(projects, &original, pwd, SAMPLE_ID)
            .expect("should succeed")
            .expect("a symlink should be created");

        assert_eq!(
            link,
            projects
                .join("-Volumes-x-here-cwd")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_here_link_returns_none_when_already_in_session_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        // encode_project_dir("/orig") == "-orig", so pwd resolves to the folder
        // the session already lives in: no symlink is needed.
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let result =
            prepare_here_link(projects, &original, Path::new("/orig"), SAMPLE_ID).expect("ok");
        assert_eq!(result, None);
        assert!(original.is_file(), "original must be left untouched");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_here_link_leaves_an_existing_link_in_place() {
        // A symlink dropped by an earlier `--here` already makes the session
        // resolvable here, so a repeat returns `None` (nothing to clean up) and
        // does not disturb the existing link.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let pwd = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let link = folder.join(format!("{SAMPLE_ID}.jsonl"));
        std::os::unix::fs::symlink(&original, &link).unwrap();

        assert_eq!(
            prepare_here_link(projects, &original, pwd, SAMPLE_ID).expect("ok"),
            None
        );
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_here_link_returns_none_when_a_real_file_is_already_present() {
        // A session id is a UUID, so a real file at the target name can only be
        // this very session living here already: resolve it in place, untouched.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let pwd = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let real = folder.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&real, "a real session").unwrap();

        assert_eq!(
            prepare_here_link(projects, &original, pwd, SAMPLE_ID).expect("ok"),
            None
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "a real session",
            "the real file must be left untouched"
        );
    }

    #[test]
    fn find_session_file_locates_jsonl_in_project_subdir() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let proj = projects.join("-Users-tim-code-foo");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();

        assert_eq!(find_session_file(projects, SAMPLE_ID), Some(file));
    }

    #[test]
    fn find_session_file_returns_none_when_absent() {
        let dir = tempdir().unwrap();
        assert_eq!(find_session_file(dir.path(), SAMPLE_ID), None);
    }

    #[test]
    fn resolve_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_session_dir(dir.path(), "../escape"),
            Err(ResolveError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_session_dir(dir.path(), SAMPLE_ID),
            Err(ResolveError::SessionNotFound)
        ));
    }

    #[test]
    fn resolve_errors_when_no_cwd_in_session() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join(format!("{SAMPLE_ID}.jsonl")), "{\"cwd\":null}\n").unwrap();

        assert!(matches!(
            resolve_session_dir(dir.path(), SAMPLE_ID),
            Err(ResolveError::NoCwdInSession)
        ));
    }

    #[test]
    fn resolve_errors_when_directory_missing() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let missing = dir.path().join("gone");
        fs::write(
            proj.join(format!("{SAMPLE_ID}.jsonl")),
            cwd_line(missing.to_str().unwrap()),
        )
        .unwrap();

        match resolve_session_dir(dir.path(), SAMPLE_ID) {
            Err(ResolveError::DirectoryMissing(path)) => assert_eq!(path, missing),
            other => panic!("expected DirectoryMissing, got {other:?}"),
        }
    }

    #[test]
    fn resolve_returns_existing_directory() {
        let dir = tempdir().unwrap();
        let proj = dir.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let cwd = dir.path().join("real-cwd");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(
            proj.join(format!("{SAMPLE_ID}.jsonl")),
            cwd_line(cwd.to_str().unwrap()),
        )
        .unwrap();

        assert_eq!(resolve_session_dir(dir.path(), SAMPLE_ID).unwrap(), cwd);
    }

    /// A real session record as written by Claude Code.
    const SESSION_JSON: &str = r#"{"pid":17041,"sessionId":"3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d","cwd":"/Volumes/code/crap","startedAt":1779730239473,"version":"2.1.150","kind":"interactive","entrypoint":"cli","status":"busy","updatedAt":1779730460209}"#;

    #[test]
    fn parse_session_record_extracts_fields() {
        let rec = parse_session_record(SESSION_JSON).expect("should parse");
        assert_eq!(rec.pid, 17041);
        assert_eq!(rec.session_id, "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d");
        assert_eq!(rec.cwd, "/Volumes/code/crap");
        assert_eq!(rec.status.as_deref(), Some("busy"));
    }

    #[test]
    fn parse_session_record_rejects_malformed_or_incomplete() {
        assert_eq!(parse_session_record("not json"), None);
        assert_eq!(parse_session_record("{\"pid\":1}"), None); // no sessionId
        assert_eq!(parse_session_record("{\"sessionId\":\"x\"}"), None); // no pid
    }

    #[test]
    fn find_live_session_matches_only_alive_pid_for_id() {
        let dir = tempdir().unwrap();
        let target = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";

        // A different session, alive — must be ignored.
        fs::write(
            dir.path().join("100.json"),
            serde_json::json!({"pid":100,"sessionId":"other","cwd":"/x"}).to_string(),
        )
        .unwrap();
        // The target session — written with pid 17041.
        fs::write(dir.path().join("17041.json"), SESSION_JSON).unwrap();

        // Nothing alive -> no live session.
        assert_eq!(find_live_session(dir.path(), target, |_| false), None);

        // Only the target pid alive -> found.
        let found = find_live_session(dir.path(), target, |pid| pid == 17041)
            .expect("target session should be found");
        assert_eq!(found.pid, 17041);
        assert_eq!(found.cwd, "/Volumes/code/crap");
    }

    #[test]
    fn find_live_session_ignores_stale_record_for_target() {
        let dir = tempdir().unwrap();
        let target = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";
        fs::write(dir.path().join("17041.json"), SESSION_JSON).unwrap();

        // The matching record exists but its pid is dead (process exited
        // uncleanly leaving the file behind) -> not a live session.
        assert_eq!(find_live_session(dir.path(), target, |_| false), None);
    }

    #[test]
    fn find_live_session_missing_dir_is_none() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-sessions-here");
        assert_eq!(find_live_session(&missing, "anything", |_| true), None);
    }

    #[test]
    fn output_emits_session_id_before_directory() {
        // The session id (a validated UUID) goes on the first line; the
        // directory comes last so any newline inside a path can't be mistaken
        // for the end of the session-id field.
        let weird_dir = Path::new("/Users/tim/od\nd\u{2009}dir");
        let out = format_output(weird_dir, SAMPLE_ID);

        assert_eq!(out.lines().next(), Some(SAMPLE_ID));
        // Everything after the first newline is the directory, intact.
        let rest = out.split_once('\n').map(|(_, rest)| rest).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), weird_dir.to_str().unwrap());
    }

    #[test]
    fn resolve_here_link_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_here_link(dir.path(), Path::new("/x"), "../escape"),
            Err(HereResolveError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_here_link_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_here_link(dir.path(), Path::new("/x"), SAMPLE_ID),
            Err(HereResolveError::SessionNotFound)
        ));
    }

    #[test]
    fn resolve_here_link_links_an_existing_session_into_pwd() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let link = resolve_here_link(projects, Path::new("/Volumes/x/here"), SAMPLE_ID)
            .expect("ok")
            .expect("a symlink should be created");
        assert_eq!(
            link,
            projects
                .join("-Volumes-x-here")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn resolve_here_link_returns_none_when_session_already_here() {
        // The session already lives in the current directory's folder, so no
        // symlink is needed and there is nothing to clean up afterwards.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("{SAMPLE_ID}.jsonl")), "{}\n").unwrap();

        assert_eq!(
            resolve_here_link(projects, Path::new("/Volumes/x/here"), SAMPLE_ID).expect("ok"),
            None
        );
    }

    #[test]
    fn shell_code_detects_here_sentinel_from_the_binary() {
        // The shell function branches on the exact sentinel the binary emits.
        assert!(SHELL_CODE.contains(HERE_SENTINEL));
    }

    #[test]
    fn shell_code_forks_and_cleans_up_in_here_mode() {
        // here-mode forks a fresh session instead of appending to the original
        // transcript...
        assert!(SHELL_CODE.contains("--fork-session"));
        // ...and removes the temporary symlink afterwards, unless there was
        // none to remove.
        assert!(SHELL_CODE.contains(r#"rm -f -- "$__crap_link""#));
        assert!(SHELL_CODE.contains(NO_LINK_SENTINEL));
    }

    #[test]
    fn here_output_carries_sentinel_session_and_link() {
        let link = Path::new("/Users/tim/.claude/projects/-x/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, Some(link));

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(HERE_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        // Everything after the second newline is the link path, intact.
        let rest = out.splitn(3, '\n').nth(2).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn here_output_uses_no_link_sentinel_when_nothing_to_clean() {
        let out = format_here_output(SAMPLE_ID, None);

        assert_eq!(out.lines().next(), Some(HERE_SENTINEL));
        let link_field = out.splitn(3, '\n').nth(2).unwrap();
        assert_eq!(link_field.trim_end_matches('\n'), NO_LINK_SENTINEL);
    }

    #[test]
    fn here_output_preserves_newline_in_link_path() {
        // The link lives last in the output, so a newline inside the path can't
        // be mistaken for a field boundary.
        let link = Path::new("/Users/tim/od\ndd/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, Some(link));

        let rest = out.splitn(3, '\n').nth(2).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn shell_code_guards_cd_against_dash_prefixed_dirs() {
        // `cd -- "$dir"` stops option parsing, so a directory whose name begins
        // with '-' is treated as a path rather than a flag.
        assert!(SHELL_CODE.contains("cd -- \"$__crap_dir\""));
    }

    #[test]
    fn shell_code_defines_function_and_dispatches_to_claude() {
        assert!(SHELL_CODE.contains("function crap()"));
        // Forwards all args so --force reaches the binary.
        assert!(SHELL_CODE.contains("command crap \"$@\""));
        // Splits the output on the first newline: session id leads, directory
        // is the remainder (so a path with embedded newlines stays whole).
        assert!(SHELL_CODE.contains("__crap_session=${__crap_out%%$'\\n'*}"));
        assert!(SHELL_CODE.contains("__crap_dir=${__crap_out#*$'\\n'}"));
        assert!(SHELL_CODE.contains("clauded --resume"));
        assert!(SHELL_CODE.contains("claude --resume"));
    }
}
