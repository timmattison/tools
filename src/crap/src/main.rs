//! crap — "Claude, Resume Anywhere Please".
//!
//! Resume a Claude Code session from whatever directory it was originally
//! started in, no matter where you are now. Given a session id, `crap` looks
//! up that session under `~/.claude/projects`, recovers the directory it ran
//! in, changes into it, and re-launches Claude with `--resume <id>`.
//!
//! Because a binary cannot change its parent shell's working directory (nor see
//! shell aliases such as `clauded`), the user-facing `crap` command is a shell
//! function installed via `crap --shell-setup`. This binary's job is to resolve
//! a session id to its original directory and print that path to stdout; the
//! shell function performs the `cd` and runs `clauded`/`claude` from there.

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

/// Returns the `~/.claude/projects` directory, or `None` if the home directory
/// cannot be determined.
fn claude_projects_dir() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".claude").join("projects"))
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
/// Emits two parts separated by a newline. The shell function `cd`s into the
/// directory and resumes the session id.
fn format_output(dir: &Path, session_id: &str) -> String {
    format!("{}\n{}\n", dir.display(), session_id)
}

/// The shell function installed by `crap --shell-setup`.
///
/// `crap` shadows the binary, so the function reaches the binary explicitly via
/// `command crap`, forwarding all arguments (so flags like `--force` work). The
/// binary prints two lines on success — the resolved directory and the session
/// id — which the function reads back; it then `cd`s there and re-launches
/// Claude. `clauded` is resolved through `eval` so that an alias of that name is
/// expanded at call time (shell aliases are otherwise not expanded inside
/// function bodies); if no `clauded` exists, plain `claude` is used. If the
/// binary exits non-zero (session not found, already running, …) its message is
/// shown and the function stops without changing directory.
const SHELL_CODE: &str = r#"
function crap() {
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    local __crap_dir __crap_session
    {
        IFS= read -r __crap_dir
        IFS= read -r __crap_session
    } <<EOF
$__crap_out
EOF
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

    /// Install the `crap` shell function into your shell config, then exit.
    ///
    /// Run this once: `crap --shell-setup`. After re-sourcing your shell,
    /// `crap <session-id>` will cd into the session's directory and resume it.
    #[arg(long)]
    shell_setup: bool,
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

    match resolve_session_dir(&projects_dir, &session_id) {
        Ok(dir) => {
            if !cli.force {
                if let Some(live) = claude_sessions_dir()
                    .and_then(|s| find_live_session(&s, &session_id, pid_is_alive))
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
    fn shell_code_guards_cd_against_dash_prefixed_dirs() {
        // `cd -- "$dir"` stops option parsing, so a directory whose name begins
        // with '-' is treated as a path rather than a flag.
        assert!(SHELL_CODE.contains("cd -- \"$__crap_dir\""));
    }

    #[test]
    fn shell_code_defines_function_and_dispatches_to_claude() {
        assert!(SHELL_CODE.contains("function crap()"));
        // Forwards all args (so --force reaches the binary) and reads back the
        // two-line output (directory, then session id).
        assert!(SHELL_CODE.contains("command crap \"$@\""));
        assert!(SHELL_CODE.contains("read -r __crap_dir"));
        assert!(SHELL_CODE.contains("read -r __crap_session"));
        assert!(SHELL_CODE.contains("clauded --resume"));
        assert!(SHELL_CODE.contains("claude --resume"));
    }
}
