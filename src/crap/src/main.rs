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
//! original transcript untouched. Because the fork only reads that transcript,
//! `--here` works even while the original session is still live in another
//! process. The symlink is removed once the session ends. A second argument
//! (`crap --here <id> <new-id>`) pins the fork to a chosen UUID via
//! `claude --session-id` instead of a random one, provided it does not already
//! name an existing session.
//!
//! With `--status`, it resumes nothing: it classifies where the session left
//! off — `waiting-for-user`, `busy`, `awaiting-assistant`, or `empty`, inferred
//! from the last conversational turn in the transcript (or the live process's
//! own status when one is attached) — and prints that one scriptable token.
//! Given no id, `--status` instead lists every session recorded for the current
//! directory, each with its state and the times its transcript was started and
//! last written (read from the transcript's own timestamps, not file mtimes).
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
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use serde::Serialize;
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
    /// `--here`: the requested new session id already names a transcript.
    pub const NEW_SESSION_ID_EXISTS: i32 = 10;
}

/// Why a located session transcript could not be resolved to an existing
/// directory. (Id validity is checked separately, before a transcript is even
/// located, so it is not represented here.)
#[derive(Debug)]
enum ResolveError {
    /// The transcript could not be read.
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

/// One user's `~/.claude/projects` directory, tagged with who owns it.
///
/// Cross-user discovery searches an ordered list of these roots; the `is_self`
/// flag lets the resume logic pick "resume in place" (the current user's own
/// session) versus "copy into my tree and fork" (another user's session).
#[derive(Debug, Clone, PartialEq, Eq)]
struct UserProjects {
    /// The account name (the home directory's file name).
    user: String,
    /// That user's `.../.claude/projects` directory.
    projects_dir: PathBuf,
    /// Whether this is the invoking user's own tree.
    is_self: bool,
}

/// The current user's own `~/.claude/projects` root (`is_self = true`).
///
/// `home` is passed explicitly rather than read from the environment here, so
/// the mapping stays tempdir-testable; the one place that reads
/// `dirs::home_dir()` is `main`.
fn self_projects(home: &Path) -> UserProjects {
    let user = home
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string();
    UserProjects {
        user,
        projects_dir: home.join(".claude").join("projects"),
        is_self: true,
    }
}

/// A sibling user's `~/.claude/projects` root, resolved as `users_parent/name`.
///
/// `is_self` is set when `name` equals `self_name` (the invoking user), so a
/// `--user` that names the current account is treated as a same-user hit. Like
/// [`self_projects`], every input is explicit so the mapping is tempdir-testable.
#[allow(
    dead_code,
    reason = "consumed by the cross-user resume wiring in a later commit"
)]
fn user_projects(users_parent: &Path, name: &str, self_name: &str) -> UserProjects {
    UserProjects {
        user: name.to_string(),
        projects_dir: users_parent.join(name).join(".claude").join("projects"),
        is_self: name == self_name,
    }
}

/// The outcome of searching an ordered list of roots for a session id.
#[derive(Debug)]
enum FoundSession {
    /// The transcript, and which root (hence user / `is_self`) it came from.
    Found {
        /// The `<id>.jsonl` transcript that matched.
        path: PathBuf,
        /// The root it was found under, carrying the owning user and `is_self`.
        root: UserProjects,
    },
    /// The id was not found in any of the searched roots.
    NotFound,
}

/// Searches an ordered list of roots for a session id, first match winning.
///
/// Within each root it reuses [`find_session_file`] as the per-root inner loop,
/// so a hit is tagged with the root it came from (hence its owning user and
/// whether it is the current user's own tree). Roots are searched in order and
/// the search short-circuits on the first match, so a self-first ordering makes
/// a session the current user already owns always win.
fn find_session_across(roots: &[UserProjects], id: &str) -> FoundSession {
    for root in roots {
        if let Some(path) = find_session_file(&root.projects_dir, id) {
            return FoundSession::Found {
                path,
                root: root.clone(),
            };
        }
    }
    FoundSession::NotFound
}

/// The conversational state of a session, inferred from its transcript.
///
/// Claude Code never writes an explicit "turn finished, waiting for input"
/// marker, so the state is derived from the shape of the last *conversational*
/// turn (subagent/`isSidechain` and injected `isMeta` entries, and trailing
/// bookkeeping lines, are not turns and are ignored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    /// The transcript records no conversational turns yet.
    Empty,
    /// Claude finished its turn and is waiting for the user: the last turn is
    /// an assistant message that ended with prose, with no pending tool call.
    WaitingForUser,
    /// Work is in flight — the assistant has an unanswered tool call, or a tool
    /// result was just delivered and the assistant has yet to respond.
    Busy,
    /// The user sent the last message and Claude has not replied yet (an active
    /// turn, or a session abandoned before the reply).
    AwaitingAssistant,
}

impl SessionState {
    /// The stable lowercase token printed by `crap --status`, suitable for
    /// scripting.
    fn as_token(self) -> &'static str {
        match self {
            SessionState::Empty => "empty",
            SessionState::WaitingForUser => "waiting-for-user",
            SessionState::Busy => "busy",
            SessionState::AwaitingAssistant => "awaiting-assistant",
        }
    }
}

/// Reports whether an assistant turn has ended (it is now the user's turn)
/// rather than leaving a tool call pending.
///
/// `stop_reason` is authoritative when present and is replicated onto every
/// JSONL line of a message, so the message's last line carries it even when
/// that line holds only a `thinking` or `text` block. When the field is absent
/// or null — which happens when a turn was interrupted mid-stream — the type of
/// the last content block is used as a fallback: a trailing `tool_use` block
/// means a tool call is still pending.
fn assistant_turn_ended(message: &serde_json::Value) -> bool {
    match message
        .get("stop_reason")
        .and_then(serde_json::Value::as_str)
    {
        Some("tool_use") => false,
        Some("end_turn" | "stop_sequence") => true,
        _ => {
            let last_block = message
                .get("content")
                .and_then(serde_json::Value::as_array)
                .and_then(|blocks| blocks.last())
                .and_then(|block| block.get("type"))
                .and_then(serde_json::Value::as_str);
            last_block != Some("tool_use")
        }
    }
}

/// Reports whether a `user` turn carries a tool result (the model is expected
/// to respond next) rather than a genuine user prompt.
fn user_turn_is_tool_result(message: &serde_json::Value) -> bool {
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
            })
        })
}

/// Infers the [`SessionState`] from the raw contents of a session transcript.
///
/// Each line is an independent JSON object. Subagent turns (`isSidechain`) and
/// injected entries (`isMeta`) belong to other threads, and bookkeeping lines
/// (`last-prompt`, `ai-title`, `file-history-snapshot`, …) are not turns at
/// all; all are skipped. The state is decided by the last surviving
/// conversational turn — there is no explicit "waiting for input" marker in the
/// transcript to read directly.
fn classify_session_state(contents: &str) -> SessionState {
    let mut state = SessionState::Empty;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            || value.get("isMeta").and_then(serde_json::Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        state = match value.get("type").and_then(serde_json::Value::as_str) {
            Some("assistant") => {
                if assistant_turn_ended(message) {
                    SessionState::WaitingForUser
                } else {
                    SessionState::Busy
                }
            }
            Some("user") => {
                if user_turn_is_tool_result(message) {
                    SessionState::Busy
                } else {
                    SessionState::AwaitingAssistant
                }
            }
            // Any other entry type is bookkeeping, not a turn: leave the state.
            _ => continue,
        };
    }
    state
}

/// Reads the existing directory a session ran in out of its located transcript.
///
/// The transcript's first non-empty `cwd` is taken as the working directory; the
/// call fails if the transcript cannot be read, records no `cwd`, or names a
/// directory that no longer exists.
///
/// # Errors
///
/// Returns a [`ResolveError`] when the transcript cannot be read, records no
/// working directory, or that directory no longer exists.
fn session_dir_from_transcript(transcript: &Path) -> Result<PathBuf, ResolveError> {
    let contents =
        std::fs::read_to_string(transcript).map_err(|_| ResolveError::SessionNotFound)?;
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

/// How [`prepare_import`] should make a session's transcript resolvable from a
/// target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportMode {
    /// Symlink the target name to the source (same-user `--here`): the original
    /// is only ever read, and the link is cheap and trivially removable.
    Symlink,
    /// Copy the source into the target folder (cross-user): a self-contained
    /// snapshot owned by the current user, so nothing under another user's home
    /// is written and the import survives the source moving.
    Copy,
}

/// Makes the session `source_jsonl` resolvable by `claude --resume` from
/// `target_dir`, by placing it into `target_dir`'s project folder under
/// `dest_projects_dir` — either as a symlink to the original ([`ImportMode::Symlink`])
/// or as a self-contained copy ([`ImportMode::Copy`]).
///
/// `dest_projects_dir` is always the current user's tree, so every write lands
/// under the current user's home even when `source_jsonl` belongs to another
/// user.
///
/// Returns the path that was created (so the caller can remove it once the
/// session ends), or `None` when the session is *already* resolvable from
/// `target_dir` — because `target_dir` is its own directory, or an earlier
/// import already placed it. Anything already at the target name is left
/// untouched: a session id is a UUID, so a file there can only be this very
/// session, and clobbering it would never be correct.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the project folder cannot be
/// created or the symlink/copy cannot be made.
fn prepare_import(
    dest_projects_dir: &Path,
    source_jsonl: &Path,
    target_dir: &Path,
    session_id: &str,
    mode: ImportMode,
) -> std::io::Result<Option<PathBuf>> {
    let folder = dest_projects_dir.join(encode_project_dir(target_dir));
    let link_path = folder.join(format!("{session_id}.jsonl"));

    // Anything already at this name means the session resolves from here
    // already. `symlink_metadata` does not follow links, so even a dangling
    // symlink left by an earlier import counts as "present".
    if link_path.symlink_metadata().is_ok() {
        return Ok(None);
    }

    std::fs::create_dir_all(&folder)?;

    match mode {
        ImportMode::Symlink => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(source_jsonl, &link_path)?;
            #[cfg(not(unix))]
            std::os::windows::fs::symlink_file(source_jsonl, &link_path)?;
        }
        ImportMode::Copy => {
            std::fs::copy(source_jsonl, &link_path)?;
        }
    }

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
    let original =
        find_session_file(projects_dir, session_id).ok_or(HereResolveError::SessionNotFound)?;
    prepare_import(
        projects_dir,
        &original,
        pwd,
        session_id,
        ImportMode::Symlink,
    )
    .map_err(HereResolveError::Io)
}

/// A caller-supplied `--here` new-session id that is not a valid UUID.
#[derive(Debug, PartialEq, Eq)]
struct InvalidNewSessionId;

/// Validates the optional forked-session id a caller passed as the second
/// `--here` argument.
///
/// Returns `Ok(None)` when none was supplied (so Claude mints a fresh random
/// id), `Ok(Some(id))` when it is a valid UUID, and `Err(InvalidNewSessionId)`
/// when one was supplied but is malformed. Validating up front keeps a bad id
/// from ever reaching the shell function's `claude --session-id`.
///
/// # Errors
///
/// Returns [`InvalidNewSessionId`] if `new_session_id` is `Some` but not a UUID.
fn resolve_new_session_id(
    new_session_id: Option<&str>,
) -> Result<Option<&str>, InvalidNewSessionId> {
    match new_session_id {
        None => Ok(None),
        Some(id) if is_valid_session_id(id) => Ok(Some(id)),
        Some(_) => Err(InvalidNewSessionId),
    }
}

/// Whether pinning a `--here` fork to `new_session_id` would collide with an
/// existing transcript.
///
/// `claude --session-id <id>` writes to `<id>.jsonl`, so reusing an id that
/// already names a session would let the fork overwrite an unrelated
/// conversation — the opposite of `--here`'s "leave the original untouched"
/// guarantee. `None` (no forced id, Claude mints a random one) never collides.
fn new_session_id_collides(projects_dir: &Path, new_session_id: Option<&str>) -> bool {
    match new_session_id {
        Some(id) => find_session_file(projects_dir, id).is_some(),
        None => false,
    }
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

/// Why `crap --status` could not report a session's state.
#[derive(Debug)]
enum StatusError {
    /// The session id was not a valid UUID.
    InvalidSessionId,
    /// No `<session_id>.jsonl` file was found under any project directory.
    SessionNotFound,
}

/// Returns the state line for a session that is open in a live `claude`
/// process — `"<status> (live, pid <pid>)"` — or `None` when no such process is
/// attached. A live process's own reported status is more authoritative than
/// anything inferred from the transcript, so callers prefer it.
fn live_state_string<F>(sessions_dir: &Path, session_id: &str, is_alive: F) -> Option<String>
where
    F: Fn(u32) -> bool,
{
    find_live_session(sessions_dir, session_id, is_alive).map(|live| {
        let status = live.status.as_deref().unwrap_or("running");
        format!("{status} (live, pid {})", live.pid)
    })
}

/// One session's status for the per-directory listing `crap --status` prints
/// when given no id.
///
/// Serializes to camelCase JSON (`sessionId`, …) for the `--json` form;
/// `started`/`last` become `null` when no timestamp was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusReport {
    /// The session id (the `.jsonl` filename stem under the project folder).
    session_id: String,
    /// The state line: a [`SessionState`] token, or `"<status> (live, pid N)"`
    /// when a `claude` process is attached.
    state: String,
    /// The earliest `timestamp` recorded in the transcript (ISO 8601 UTC), or
    /// `None` if no line carries one.
    started: Option<String>,
    /// The latest `timestamp` recorded in the transcript (ISO 8601 UTC), or
    /// `None` if no line carries one.
    last: Option<String>,
}

/// Returns the earliest and latest `timestamp` values found in a transcript.
///
/// Claude writes ISO 8601 UTC timestamps (`…Z`, fixed width) on conversational
/// and system entries, but not on bookkeeping lines. Because the format is
/// fixed-width and always UTC, lexicographic ordering matches chronological
/// ordering, so the earliest/latest are just the string min/max — no date
/// parsing, and the result is independent of line order. Returns `(None, None)`
/// when no line carries a timestamp.
fn transcript_time_span(contents: &str) -> (Option<String>, Option<String>) {
    let mut earliest: Option<String> = None;
    let mut latest: Option<String> = None;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(ts) = value.get("timestamp").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if earliest.as_deref().is_none_or(|e| ts < e) {
            earliest = Some(ts.to_string());
        }
        if latest.as_deref().is_none_or(|l| ts > l) {
            latest = Some(ts.to_string());
        }
    }
    (earliest, latest)
}

/// Prettifies an ISO 8601 UTC timestamp (`2026-05-25T18:43:05.109Z`) into a
/// human `2026-05-25 18:43:05`, dropping the sub-second fraction and zone.
///
/// Input that does not match the expected shape is returned unchanged.
fn format_timestamp(raw: &str) -> String {
    let Some((date, rest)) = raw.split_once('T') else {
        return raw.to_string();
    };
    let time = rest.split(['.', 'Z']).next().unwrap_or(rest);
    if date.is_empty() || time.is_empty() {
        return raw.to_string();
    }
    format!("{date} {time}")
}

/// Lists the status of every session whose transcript lives in `pwd`'s project
/// folder under `projects_dir`.
///
/// This backs `crap --status` with no id: it enumerates `<uuid>.jsonl` files in
/// the folder Claude would use for `pwd`, classifying each (live process status
/// taking precedence over transcript inference) and recording its time span.
/// Results are ordered ascending by last-activity, so the most recently used
/// session is last. `is_alive` is injected so liveness can be tested without
/// spawning processes.
fn resolve_dir_statuses<F>(
    projects_dir: &Path,
    sessions_dir: &Path,
    pwd: &Path,
    is_alive: F,
) -> Vec<SessionStatusReport>
where
    F: Fn(u32) -> bool + Copy,
{
    let folder = projects_dir.join(encode_project_dir(pwd));
    let mut reports = Vec::new();
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return reports;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(session_id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_valid_session_id(session_id) {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        let state = live_state_string(sessions_dir, session_id, is_alive)
            .unwrap_or_else(|| classify_session_state(&contents).as_token().to_string());
        let (started, last) = transcript_time_span(&contents);
        reports.push(SessionStatusReport {
            session_id: session_id.to_string(),
            state,
            started,
            last,
        });
    }
    // Ascending by last-activity, so the most recently used session sits at the
    // bottom of the printed table; timestamp-less sessions (which sort first)
    // and ties break by id, keeping the order deterministic regardless of
    // directory iteration order.
    reports.sort_by(|a, b| {
        a.last
            .cmp(&b.last)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    reports
}

/// Resolves the full [`SessionStatusReport`] for a single session id.
///
/// Unlike the bare token, this also carries the transcript's time span, so the
/// JSON form of `crap --status <id>` can include start/last times. A live
/// process's status still takes precedence for the `state` field; the
/// transcript is read (best-effort) for the times either way.
///
/// # Errors
///
/// Returns [`StatusError::InvalidSessionId`] for a malformed id, or
/// [`StatusError::SessionNotFound`] when the id is neither live nor on disk.
fn resolve_status_report<F>(
    projects_dir: &Path,
    sessions_dir: &Path,
    session_id: &str,
    is_alive: F,
) -> Result<SessionStatusReport, StatusError>
where
    F: Fn(u32) -> bool,
{
    if !is_valid_session_id(session_id) {
        return Err(StatusError::InvalidSessionId);
    }
    let live = live_state_string(sessions_dir, session_id, is_alive);
    let contents =
        find_session_file(projects_dir, session_id).and_then(|f| std::fs::read_to_string(f).ok());
    let (started, last) = contents
        .as_deref()
        .map_or((None, None), transcript_time_span);
    let state = match live {
        Some(line) => line,
        None => {
            // Not live: the transcript is the only evidence, so it must exist.
            let contents = contents.ok_or(StatusError::SessionNotFound)?;
            classify_session_state(&contents).as_token().to_string()
        }
    };
    Ok(SessionStatusReport {
        session_id: session_id.to_string(),
        state,
        started,
        last,
    })
}

/// Serializes a single session's status report as pretty JSON.
fn format_status_json(report: &SessionStatusReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

/// Serializes a directory's session status reports as a pretty JSON array.
fn format_dir_statuses_json(reports: &[SessionStatusReport]) -> String {
    serde_json::to_string_pretty(reports).unwrap_or_else(|_| "[]".to_string())
}

/// Renders the per-directory `crap --status` listing for `pwd` as a table.
///
/// An empty listing reports that nothing was found; otherwise a heading line is
/// followed by a table with one row per session. A session with no recorded
/// activity shows an em-dash in its time columns.
fn format_dir_statuses(pwd: &Path, reports: &[SessionStatusReport]) -> String {
    if reports.is_empty() {
        return format!("No Claude sessions found for {}\n", pwd.display());
    }
    let count = reports.len();
    let noun = if count == 1 { "session" } else { "sessions" };
    let table = dir_statuses_table(reports);
    format!("{count} {noun} for {}\n\n{table}\n", pwd.display())
}

/// Builds the session-status table — one row per report, timestamps prettified
/// into the cells — ready for rendering.
///
/// Rendered with [`ContentArrangement::Disabled`] so each column is sized to its
/// own content and the table is laid out at its natural width, **never wrapping
/// a cell to fit the terminal**. Wrapping would chop a session UUID or timestamp
/// across lines (unreadable), and — because the dynamic arrangement reads the
/// ambient terminal width — would make the output depend on a shared,
/// uncontrolled resource, so the rendering (and any test asserting on it) would
/// silently change with the window size. Long rows simply overflow and let the
/// terminal soft-wrap.
fn dir_statuses_table(reports: &[SessionStatusReport]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Disabled)
        .set_header(vec!["SESSION", "STATE", "STARTED", "LAST"]);
    for report in reports {
        let started = report
            .started
            .as_deref()
            .map_or_else(|| "—".to_string(), format_timestamp);
        let last = report
            .last
            .as_deref()
            .map_or_else(|| "—".to_string(), format_timestamp);
        table.add_row(vec![
            report.session_id.clone(),
            report.state.clone(),
            started,
            last,
        ]);
    }
    table
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

/// Placeholder used in the forced-new-id field when `--here` was given no
/// explicit new session id, so the shell function knows to let Claude mint a
/// fresh random id (`--fork-session`) instead of pinning one (`--session-id`).
const NO_NEW_ID_SENTINEL: &str = "__CRAP_NO_NEW_ID__";

/// Leading token marking cross-user default-resume output: unlike [`HERE_SENTINEL`]
/// (which stays in the current directory), this tells the shell function to
/// `cd` into the session's original recorded directory *and then* fork, so a
/// foreign session resumes where it originally ran.
const FORK_AT_SENTINEL: &str = "__CRAP_FORK_AT__";

/// Formats `--here` output for the shell function: the [`HERE_SENTINEL`], then
/// the session id, then the caller-supplied forked-session id (or
/// [`NO_NEW_ID_SENTINEL`] when none was given), then the symlink to remove once
/// the session ends (or [`NO_LINK_SENTINEL`] when none was created).
///
/// The cleanup path is emitted last so that — like [`format_output`] — a path
/// containing a newline survives intact as "everything after the final field
/// separator". The forced-new-id is a validated UUID (or the sentinel), so it
/// never contains a newline and is safe in the middle.
fn format_here_output(
    session_id: &str,
    new_session_id: Option<&str>,
    link_to_cleanup: Option<&Path>,
) -> String {
    let new_id = new_session_id.unwrap_or(NO_NEW_ID_SENTINEL);
    let link = match link_to_cleanup {
        Some(path) => path.display().to_string(),
        None => NO_LINK_SENTINEL.to_string(),
    };
    format!("{HERE_SENTINEL}\n{session_id}\n{new_id}\n{link}\n")
}

/// Formats cross-user default-resume output for the shell function: the
/// [`FORK_AT_SENTINEL`], then the session id, then the forked-session id (or
/// [`NO_NEW_ID_SENTINEL`]), then the imported transcript to remove once the
/// session ends (or [`NO_LINK_SENTINEL`]), and finally the session's original
/// directory to `cd` into before forking.
///
/// The directory is emitted **last** so — like [`format_output`] — a path
/// containing a newline survives intact as "everything after the final field
/// separator". The middle fields are all newline-free: the ids are validated
/// UUIDs (or sentinels), and the link path lives under
/// `~/.claude/projects/<encoded>`, whose encoding maps every non-alphanumeric
/// character (including newline) to `-`.
fn format_fork_at_output(
    session_id: &str,
    new_session_id: Option<&str>,
    link_to_cleanup: Option<&Path>,
    dir: &Path,
) -> String {
    let new_id = new_session_id.unwrap_or(NO_NEW_ID_SENTINEL);
    let link = match link_to_cleanup {
        Some(path) => path.display().to_string(),
        None => NO_LINK_SENTINEL.to_string(),
    };
    format!(
        "{FORK_AT_SENTINEL}\n{session_id}\n{new_id}\n{link}\n{}\n",
        dir.display()
    )
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
/// The binary speaks one of three output shapes:
///
/// * **default** — `<session-id>\n<dir>`: the function `cd`s into the original
///   directory (splitting on the first newline so a path containing newlines
///   survives intact) and resumes.
/// * **`--here`** — `__CRAP_HERE__\n<session-id>\n<new-id-or-sentinel>\n<link-or-sentinel>`:
///   the binary has already symlinked the session into the *current* directory's
///   project folder, so the function stays put, resumes with `--fork-session` (a
///   fresh session id, leaving the original transcript untouched), and finally
///   removes that symlink — unless the link field is `__CRAP_NO_LINK__`, meaning
///   none was created because this already is the session's own directory. When
///   the new-id field is not `__CRAP_NO_NEW_ID__`, the fork is pinned to that id
///   via `--session-id` instead of a random one.
/// * **cross-user** — `__CRAP_FORK_AT__\n<session-id>\n<new-id-or-sentinel>\n<link-or-sentinel>\n<dir>`:
///   the binary has *copied* another user's transcript into our own tree, so the
///   function `cd`s into the session's original directory (the trailing `<dir>`,
///   emitted last so a newline in the path survives) and then runs the same
///   fork + cleanup sequence as `--here`.
const SHELL_CODE: &str = r#"
function crap() {
    # These flags make the binary print to stdout and exit 0 without mutating
    # the parent shell: --status queries, --help/-h/--version/-V emit
    # informational text, and --shell-setup writes the rc file (not the live
    # shell) and prints activation instructions. Run them straight through so
    # their output reaches the terminal instead of being parsed as a
    # "<session-id>\n<dir>" resume target (which would otherwise `cd` into that
    # text and mangle it). --shell-setup matters on upgrades, when this very
    # function is already loaded and would otherwise swallow its instructions.
    case " $* " in
        *" --status "*|*" --help "*|*" -h "*|*" --version "*|*" -V "*|*" --shell-setup "*)
            command crap "$@"; return $? ;;
    esac
    local __crap_out
    __crap_out=$(command crap "$@") || return $?
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_HERE__" ]; then
        local __crap_rest __crap_session __crap_newid __crap_link __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_newid=${__crap_rest%%$'\n'*}
        __crap_link=${__crap_rest#*$'\n'}
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            # Claude only needs the symlink while it reads the transcript at
            # startup; once it writes the forked session file the symlink is
            # vestigial. Watch the folder and drop it the moment a new .jsonl
            # appears, rather than letting it linger for the whole session.
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        # Build the resume argv: always --fork-session, so the original
        # transcript is left untouched. When the binary supplied a forced id
        # (third field is not the sentinel), pin the fork to it with
        # --session-id instead of letting Claude mint a random one. The earlier
        # "command crap" call has already consumed the function's own arguments,
        # so reusing the positional parameters here is safe.
        set -- --resume "$__crap_session" --fork-session
        if [ "$__crap_newid" != "__CRAP_NO_NEW_ID__" ]; then
            set -- "$@" --session-id "$__crap_newid"
        fi
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded "$@"'
        else
            claude "$@"
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
            rm -f -- "$__crap_link"
        fi
        return
    fi
    if [ "${__crap_out%%$'\n'*}" = "__CRAP_FORK_AT__" ]; then
        # Cross-user resume: the binary copied a foreign transcript into our own
        # tree and wants it forked at the session's ORIGINAL directory. The wire
        # shape adds a trailing <dir> field to the here-mode layout —
        # "__CRAP_FORK_AT__\n<session>\n<new-id>\n<link>\n<dir>" — with <dir>
        # last so a path containing newlines survives as the final field. We cd
        # there, then run the same fork + cleanup sequence as --here.
        local __crap_rest __crap_session __crap_newid __crap_link __crap_dir __crap_folder __crap_n0 __crap_watcher
        __crap_rest=${__crap_out#*$'\n'}
        __crap_session=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_newid=${__crap_rest%%$'\n'*}
        __crap_rest=${__crap_rest#*$'\n'}
        __crap_link=${__crap_rest%%$'\n'*}
        __crap_dir=${__crap_rest#*$'\n'}
        cd -- "$__crap_dir" || return 1
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            # As in --here: drop the imported copy the moment Claude writes the
            # forked session file, rather than letting it linger.
            __crap_folder=$(dirname -- "$__crap_link")
            __crap_n0=$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')
            (
                __crap_i=0
                while [ "$__crap_i" -lt 600 ]; do
                    if [ "$(find "$__crap_folder" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l | tr -dc '0-9')" -gt "$__crap_n0" ]; then
                        rm -f -- "$__crap_link"
                        exit 0
                    fi
                    __crap_i=$((__crap_i + 1))
                    sleep 0.1
                done
            ) &
            __crap_watcher=$!
            disown 2>/dev/null
        fi
        set -- --resume "$__crap_session" --fork-session
        if [ "$__crap_newid" != "__CRAP_NO_NEW_ID__" ]; then
            set -- "$@" --session-id "$__crap_newid"
        fi
        if command -v clauded >/dev/null 2>&1; then
            eval 'clauded "$@"'
        else
            claude "$@"
        fi
        if [ "$__crap_link" != "__CRAP_NO_LINK__" ]; then
            kill "$__crap_watcher" 2>/dev/null
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
    let integration = ShellIntegration::new("crap", "Claude, Resume Anywhere Please", SHELL_CODE)
        .with_command(
            "crap",
            "Resume a Claude session from its original directory",
        );
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
    ///
    /// Optional with `--status`: given no id, `--status` lists every session
    /// recorded for the current directory instead of resolving one.
    #[arg(
        value_name = "SESSION_ID",
        required_unless_present_any = ["shell_setup", "status"]
    )]
    session_id: Option<String>,

    /// Id to assign the forked session created by `--here` (must be a UUID).
    ///
    /// Only valid with `--here`. When given, the resumed fork is created with
    /// this exact id (`claude --fork-session --session-id <id>`) instead of a
    /// random one — useful when a caller needs to know the new id in advance.
    /// Without it, Claude mints a fresh random id as before.
    #[arg(value_name = "NEW_SESSION_ID", requires = "here")]
    new_session_id: Option<String>,

    /// Resume even if the session appears to be running in another process.
    ///
    /// By default `crap` refuses to resume a session that is already open
    /// elsewhere, because two processes writing the same session log can
    /// corrupt it.
    ///
    /// This guard applies only to the default resume mode. `--here` forks a
    /// fresh session (it only reads the original transcript), so it is never
    /// blocked by a live original and ignores `--force`.
    #[arg(short, long)]
    force: bool,

    /// Resume the session in the *current* directory instead of its original.
    ///
    /// `crap` symlinks the session into the current directory's project folder
    /// so `claude --resume` can find it here, then resumes it as a forked
    /// (new-id) session — the original transcript is only read, never written,
    /// so this works even if the original session is still live. Use this to
    /// carry a conversation's context into a different working directory.
    #[arg(long)]
    here: bool,

    /// Print the session's conversational state and exit, without resuming.
    ///
    /// With a session id, emits one scriptable token: `waiting-for-user`
    /// (Claude finished and is waiting on you), `busy` (a tool call or reply is
    /// in flight), `awaiting-assistant` (you spoke last and Claude hasn't
    /// replied), or `empty`. If the session is currently open in a live `claude`
    /// process, its own status is reported instead, as
    /// `<status> (live, pid <pid>)`.
    ///
    /// With no id, lists every session recorded for the current directory —
    /// each with its state and the times its transcript was started and last
    /// written.
    #[arg(long)]
    status: bool,

    /// Emit machine-readable JSON instead of human-formatted text.
    ///
    /// Only valid with `--status`. With a session id it prints one object;
    /// with no id it prints an array of one object per session. Timestamps are
    /// the raw ISO 8601 values from the transcript.
    #[arg(long, requires = "status")]
    json: bool,

    /// Install the `crap` shell function into your shell config, then exit.
    ///
    /// Run this once: `crap --shell-setup`. After re-sourcing your shell,
    /// `crap <session-id>` will cd into the session's directory and resume it.
    #[arg(long)]
    shell_setup: bool,
}

/// Whether a session that is already live in another process should block a
/// resume of it.
///
/// The default resume mode reuses the session id, so a second process would
/// append to the same transcript as the live one — that can corrupt it, so a
/// live session blocks the resume unless `--force` overrides it. `--here`
/// resumes with `--fork-session`, which only *reads* the original transcript
/// and writes a fresh file, so a live original can never be corrupted by it —
/// `--here` therefore never blocks (and `--force` is irrelevant to it).
fn should_block_for_live(here: bool, force: bool) -> bool {
    !here && !force
}

/// Aborts with a clear message if `session_id` is already open in another live
/// process and [`should_block_for_live`] says that should block this resume.
fn abort_if_session_live(session_id: &str, here: bool, force: bool) {
    if !should_block_for_live(here, force) {
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

/// Handles `crap --here <id> [<new-id>]`: symlink the session into the current
/// directory's project folder and emit the here-mode output the shell function
/// consumes, optionally pinning the forked session's id to `new_session_id`.
fn run_here(projects_dir: &Path, session_id: &str, new_session_id: Option<&str>, force: bool) -> ! {
    let Ok(pwd) = std::env::current_dir() else {
        eprintln!(
            "{} could not determine the current directory",
            "Error:".red().bold()
        );
        exit(exit_codes::HERE_PWD_UNAVAILABLE);
    };

    // Validate the optional forced id before creating anything, so a bad id
    // aborts without leaving a stray symlink behind.
    let new_id = match resolve_new_session_id(new_session_id) {
        Ok(id) => id,
        Err(InvalidNewSessionId) => {
            eprintln!(
                "{} '{}' is not a valid session id",
                "Error:".red().bold(),
                new_session_id.unwrap_or_default()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
    };

    // Refuse to pin the fork to an id that already names a transcript: that
    // would let `claude --session-id` overwrite an unrelated session.
    if new_session_id_collides(projects_dir, new_id) {
        eprintln!(
            "{} a session with id '{}' already exists",
            "Error:".red().bold(),
            new_id.unwrap_or_default()
        );
        eprintln!("       choose a fresh id so the fork does not overwrite it");
        exit(exit_codes::NEW_SESSION_ID_EXISTS);
    }

    // Guard before creating anything, so an aborted resume leaves no stray link.
    abort_if_session_live(session_id, true, force);

    match resolve_here_link(projects_dir, &pwd, session_id) {
        Ok(link) => {
            print!(
                "{}",
                format_here_output(session_id, new_id, link.as_deref())
            );
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

/// Handles `crap --status <id>`: print the session's state to stdout and exit.
///
/// Prints the bare state token by default, or the full report as JSON when
/// `json` is set.
fn run_status(projects_dir: &Path, session_id: &str, json: bool) -> ! {
    let sessions_dir = claude_sessions_dir().unwrap_or_default();
    match resolve_status_report(projects_dir, &sessions_dir, session_id, pid_is_alive) {
        Ok(report) => {
            if json {
                println!("{}", format_status_json(&report));
            } else {
                println!("{}", report.state);
            }
            exit(0);
        }
        Err(StatusError::InvalidSessionId) => {
            eprintln!(
                "{} '{session_id}' is not a valid session id",
                "Error:".red().bold()
            );
            exit(exit_codes::INVALID_SESSION_ID);
        }
        Err(StatusError::SessionNotFound) => {
            eprintln!(
                "{} no Claude session found with id '{session_id}'",
                "Error:".red().bold()
            );
            eprintln!("       looked under {}", projects_dir.display());
            exit(exit_codes::SESSION_NOT_FOUND);
        }
    }
}

/// Handles `crap --status` with no id: list every session recorded for the
/// current directory, then exit.
///
/// Prints a table by default, or a JSON array when `json` is set.
fn run_dir_status(projects_dir: &Path, json: bool) -> ! {
    let Ok(pwd) = std::env::current_dir() else {
        eprintln!(
            "{} could not determine the current directory",
            "Error:".red().bold()
        );
        exit(exit_codes::HERE_PWD_UNAVAILABLE);
    };
    let sessions_dir = claude_sessions_dir().unwrap_or_default();
    let reports = resolve_dir_statuses(projects_dir, &sessions_dir, &pwd, pid_is_alive);
    if json {
        println!("{}", format_dir_statuses_json(&reports));
    } else {
        print!("{}", format_dir_statuses(&pwd, &reports));
    }
    exit(0);
}

/// Handles the default resume (`crap <id> [--user X]`): locate the session
/// across `roots`, then resume it and exit.
///
/// A hit in the current user's own tree (`is_self`) resumes in place — `cd` to
/// the recorded directory and `claude --resume <id>` — exactly as before. A hit
/// in another user's tree is copied into the current user's tree
/// (`dest_projects_dir`, always our own) and forked at its original directory:
/// the foreign transcript is only ever read, and every write lands under the
/// current user's home. Emits the output the shell function consumes.
fn run_resume(
    roots: &[UserProjects],
    dest_projects_dir: &Path,
    session_id: &str,
    force: bool,
) -> ! {
    if !is_valid_session_id(session_id) {
        eprintln!(
            "{} '{session_id}' is not a valid session id",
            "Error:".red().bold()
        );
        exit(exit_codes::INVALID_SESSION_ID);
    }

    let FoundSession::Found { path, root } = find_session_across(roots, session_id) else {
        eprintln!(
            "{} no Claude session found with id '{session_id}'",
            "Error:".red().bold()
        );
        for root in roots {
            eprintln!("       looked under {}", root.projects_dir.display());
        }
        exit(exit_codes::SESSION_NOT_FOUND);
    };

    let dir = match session_dir_from_transcript(&path) {
        Ok(dir) => dir,
        Err(ResolveError::SessionNotFound) => {
            eprintln!(
                "{} no Claude session found with id '{session_id}'",
                "Error:".red().bold()
            );
            exit(exit_codes::SESSION_NOT_FOUND);
        }
        Err(ResolveError::NoCwdInSession) => {
            eprintln!(
                "{} session '{session_id}' has no recorded working directory",
                "Error:".red().bold()
            );
            exit(exit_codes::NO_CWD_IN_SESSION);
        }
        Err(ResolveError::DirectoryMissing(missing)) => {
            eprintln!(
                "{} the directory for session '{session_id}' no longer exists:",
                "Error:".red().bold()
            );
            eprintln!("       {}", missing.display());
            exit(exit_codes::DIRECTORY_MISSING);
        }
    };

    if root.is_self {
        // Same-user hit: resume the very session in place, as today.
        abort_if_session_live(session_id, false, force);
        print!("{}", format_output(&dir, session_id));
        exit(0);
    }

    // Cross-user hit: copy the foreign transcript into our own tree at the
    // original directory's project folder, then fork it there. The fork only
    // reads the copy, so a live original is never blocked and never corrupted.
    match prepare_import(dest_projects_dir, &path, &dir, session_id, ImportMode::Copy) {
        Ok(link) => {
            print!(
                "{}",
                format_fork_at_output(session_id, None, link.as_deref(), &dir)
            );
            exit(0);
        }
        Err(err) => {
            eprintln!(
                "{} could not import session '{session_id}': {err}",
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

    let Some(home) = dirs::home_dir() else {
        eprintln!(
            "{} could not determine your home directory",
            "Error:".red().bold()
        );
        exit(exit_codes::NO_HOME_DIR);
    };
    let projects_dir = home.join(".claude").join("projects");

    if cli.status {
        match cli.session_id.as_deref() {
            Some(id) => run_status(&projects_dir, id, cli.json),
            None => run_dir_status(&projects_dir, cli.json),
        }
    }

    // The clap `required_unless_present_any` guarantees an id is present once we
    // are past the `--shell-setup` and `--status` paths above.
    let session_id = cli
        .session_id
        .expect("session id is required without --shell-setup or --status");

    if cli.here {
        run_here(
            &projects_dir,
            &session_id,
            cli.new_session_id.as_deref(),
            cli.force,
        );
    }

    let roots = vec![self_projects(&home)];
    run_resume(&roots, &projects_dir, &session_id, cli.force);
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
    fn default_mode_blocks_a_live_session_unless_forced() {
        // Default resume reuses the session id, so a live original must block.
        assert!(should_block_for_live(false, false));
    }

    #[test]
    fn force_overrides_the_live_block_in_default_mode() {
        assert!(!should_block_for_live(false, true));
    }

    #[test]
    fn resolve_new_session_id_accepts_a_valid_uuid() {
        // A well-formed UUID is passed through so `--here` can pin the fork.
        assert_eq!(resolve_new_session_id(Some(ID_B)), Ok(Some(ID_B)));
    }

    #[test]
    fn resolve_new_session_id_absent_is_none() {
        // No second argument means Claude mints a fresh random id, as before.
        assert_eq!(resolve_new_session_id(None), Ok(None));
    }

    #[test]
    fn resolve_new_session_id_rejects_a_non_uuid() {
        // A malformed id must be caught before it reaches `claude --session-id`.
        assert_eq!(
            resolve_new_session_id(Some("not-a-uuid")),
            Err(InvalidNewSessionId)
        );
    }

    #[test]
    fn here_accepts_session_and_new_id_positionals() {
        let cli = Cli::try_parse_from(["crap", "--here", ID_A, ID_B]).expect("should parse");
        assert!(cli.here);
        assert_eq!(cli.session_id.as_deref(), Some(ID_A));
        assert_eq!(cli.new_session_id.as_deref(), Some(ID_B));
    }

    #[test]
    fn new_session_id_positional_requires_here() {
        // A forked id is meaningless without --here, so clap must reject it.
        assert!(Cli::try_parse_from(["crap", ID_A, ID_B]).is_err());
    }

    #[test]
    fn new_session_id_collides_when_the_id_already_exists() {
        // Pinning a fork to an id that already names a transcript would let it
        // overwrite that conversation, so it must be reported as a collision.
        let projects = tempdir().unwrap();
        let folder = projects.path().join("some-project");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(format!("{ID_B}.jsonl")), "{}\n").unwrap();
        assert!(new_session_id_collides(projects.path(), Some(ID_B)));
    }

    #[test]
    fn new_session_id_does_not_collide_when_unused_or_absent() {
        let projects = tempdir().unwrap();
        fs::create_dir_all(projects.path().join("some-project")).unwrap();
        // An id no transcript uses is free, and "no forced id" never collides.
        assert!(!new_session_id_collides(projects.path(), Some(ID_B)));
        assert!(!new_session_id_collides(projects.path(), None));
    }

    #[test]
    fn here_mode_never_blocks_a_live_session() {
        // `--here` resumes with `--fork-session`: it only reads the original
        // transcript and writes a fresh file, so a live original can never be
        // corrupted by it. A live session must therefore not block `--here`.
        assert!(!should_block_for_live(true, false));
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

    /// Builds an `assistant` transcript line with the given content blocks and
    /// `stop_reason` (pass `serde_json::Value::Null` for an interrupted turn).
    fn assistant_line(content: serde_json::Value, stop_reason: serde_json::Value) -> String {
        format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "message": { "stop_reason": stop_reason, "content": content },
            })
        )
    }

    /// Builds a `user` transcript line carrying the given content.
    fn user_line(content: serde_json::Value) -> String {
        format!(
            "{}\n",
            serde_json::json!({ "type": "user", "message": { "content": content } })
        )
    }

    /// A single content block of the given `type`.
    fn block(kind: &str) -> serde_json::Value {
        serde_json::json!([{ "type": kind }])
    }

    #[test]
    fn classify_empty_input_is_empty() {
        assert_eq!(classify_session_state(""), SessionState::Empty);
    }

    #[test]
    fn classify_only_bookkeeping_is_empty() {
        // Lines that are not conversational turns never establish a state.
        let contents = format!(
            "{}{}{}",
            "{\"type\":\"file-history-snapshot\"}\n",
            "{\"type\":\"last-prompt\"}\n",
            "{\"type\":\"ai-title\"}\n",
        );
        assert_eq!(classify_session_state(&contents), SessionState::Empty);
    }

    #[test]
    fn classify_assistant_end_turn_waits_for_user() {
        let contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_assistant_stop_sequence_waits_for_user() {
        let contents = assistant_line(block("text"), serde_json::json!("stop_sequence"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_assistant_pending_tool_use_is_busy() {
        let contents = assistant_line(block("tool_use"), serde_json::json!("tool_use"));
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_uses_stop_reason_over_block_type() {
        // `stop_reason` is replicated onto every line of a message, so a line
        // holding only a `thinking` block still carries the turn's real
        // `stop_reason`. A tool_use stop means a tool call is pending → busy,
        // even though this line's block is not itself a tool_use.
        let contents = assistant_line(block("thinking"), serde_json::json!("tool_use"));
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_interrupted_text_turn_waits_for_user() {
        // A null stop_reason means the turn was cut off mid-stream; with no
        // pending tool call (last block is text) the user is still up next.
        let contents = assistant_line(block("text"), serde_json::Value::Null);
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_interrupted_tool_use_is_busy() {
        // Interrupted, but the last block is a tool_use awaiting a result → busy.
        let contents = assistant_line(block("tool_use"), serde_json::Value::Null);
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_tool_result_is_busy() {
        // A tool result was just delivered; the assistant has yet to respond.
        let contents = format!(
            "{}{}",
            assistant_line(block("tool_use"), serde_json::json!("tool_use")),
            user_line(block("tool_result")),
        );
        assert_eq!(classify_session_state(&contents), SessionState::Busy);
    }

    #[test]
    fn classify_user_prompt_string_awaits_assistant() {
        // A real user prompt (string content) with no assistant reply yet.
        let contents = user_line(serde_json::json!("please continue"));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::AwaitingAssistant
        );
    }

    #[test]
    fn classify_user_prompt_text_block_awaits_assistant() {
        let contents = user_line(serde_json::json!([{ "type": "text", "text": "hi" }]));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::AwaitingAssistant
        );
    }

    #[test]
    fn classify_ignores_meta_user_entries() {
        // An injected `isMeta` user entry (system reminder, command output) is
        // not the user speaking, so the prior assistant end-of-turn wins.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "type": "user",
                "isMeta": true,
                "message": { "content": [{ "type": "text", "text": "<reminder>" }] },
            })
        ));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_ignores_sidechain_turns() {
        // Subagent (`isSidechain`) turns are interleaved in the same file but
        // belong to a different thread, so they never set the main-thread state.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "isSidechain": true,
                "message": { "stop_reason": "tool_use", "content": [{ "type": "tool_use" }] },
            })
        ));
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_ignores_trailing_bookkeeping() {
        // Bookkeeping lines are appended after the conversation; they must not
        // override the last real turn's state.
        let mut contents = assistant_line(block("text"), serde_json::json!("end_turn"));
        contents.push_str("{\"type\":\"file-history-snapshot\"}\n");
        contents.push_str("{\"type\":\"last-prompt\"}\n");
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn classify_follows_a_realistic_sequence() {
        // user prompt → assistant tool_use → tool_result → assistant end_turn:
        // Claude has finished and is waiting for the user.
        let contents = format!(
            "{}{}{}{}",
            user_line(serde_json::json!("do the thing")),
            assistant_line(block("tool_use"), serde_json::json!("tool_use")),
            user_line(block("tool_result")),
            assistant_line(block("text"), serde_json::json!("end_turn")),
        );
        assert_eq!(
            classify_session_state(&contents),
            SessionState::WaitingForUser
        );
    }

    #[test]
    fn session_state_tokens_are_stable() {
        assert_eq!(SessionState::Empty.as_token(), "empty");
        assert_eq!(SessionState::WaitingForUser.as_token(), "waiting-for-user");
        assert_eq!(SessionState::Busy.as_token(), "busy");
        assert_eq!(
            SessionState::AwaitingAssistant.as_token(),
            "awaiting-assistant"
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
        assert!(!is_valid_session_id(
            "4733ee2a-1ad6-4619-a01a-11840b8e19011"
        )); // too long
        assert!(!is_valid_session_id("4733ee2a1ad64619a01a11840b8e1901")); // no hyphens
        assert!(!is_valid_session_id("4733ee2g-1ad6-4619-a01a-11840b8e1901")); // 'g' not hex
        assert!(!is_valid_session_id(
            "4733ee2a-1ad6-4619-a01a-11840b8e1901 ; rm -rf ~"
        ));
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
    fn prepare_import_symlink_creates_symlink_in_target_project_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        // A target dir whose encoded project folder does not exist yet.
        let target = Path::new("/Volumes/x/here-cwd");
        let link = prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
            .expect("should succeed")
            .expect("a symlink should be created");

        assert_eq!(
            link,
            projects
                .join("-Volumes-x-here-cwd")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_import_symlink_returns_none_when_already_in_session_folder() {
        let dir = tempdir().unwrap();
        let projects = dir.path();
        // encode_project_dir("/orig") == "-orig", so target resolves to the
        // folder the session already lives in: no symlink is needed.
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let result = prepare_import(
            projects,
            &original,
            Path::new("/orig"),
            SAMPLE_ID,
            ImportMode::Symlink,
        )
        .expect("ok");
        assert_eq!(result, None);
        assert!(original.is_file(), "original must be left untouched");
    }

    #[cfg(unix)]
    #[test]
    fn prepare_import_symlink_leaves_an_existing_link_in_place() {
        // A symlink dropped by an earlier `--here` already makes the session
        // resolvable here, so a repeat returns `None` (nothing to clean up) and
        // does not disturb the existing link.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let target = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let link = folder.join(format!("{SAMPLE_ID}.jsonl"));
        std::os::unix::fs::symlink(&original, &link).unwrap();

        assert_eq!(
            prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
                .expect("ok"),
            None
        );
        assert_eq!(fs::read_link(&link).unwrap(), original);
    }

    #[test]
    fn prepare_import_symlink_returns_none_when_a_real_file_is_already_present() {
        // A session id is a UUID, so a real file at the target name can only be
        // this very session living here already: resolve it in place, untouched.
        let dir = tempdir().unwrap();
        let projects = dir.path();
        let orig = projects.join("-orig");
        fs::create_dir_all(&orig).unwrap();
        let original = orig.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&original, "{}\n").unwrap();

        let target = Path::new("/Volumes/x/here");
        let folder = projects.join("-Volumes-x-here");
        fs::create_dir_all(&folder).unwrap();
        let real = folder.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&real, "a real session").unwrap();

        assert_eq!(
            prepare_import(projects, &original, target, SAMPLE_ID, ImportMode::Symlink)
                .expect("ok"),
            None
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "a real session",
            "the real file must be left untouched"
        );
    }

    #[test]
    fn prepare_import_copy_snapshots_source_and_is_idempotent() {
        // Cross-user import copies the transcript into the current user's tree:
        // a self-contained regular file (not a symlink) with identical bytes,
        // and a repeat import is a no-op.
        let dir = tempdir().unwrap();
        let projects = dir.path().join("dest/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        // The source lives under a *different* tree (another user's home).
        let src_dir = dir.path().join("other/.claude/projects/-orig");
        fs::create_dir_all(&src_dir).unwrap();
        let source = src_dir.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&source, "{\"cwd\":\"/work\"}\n").unwrap();

        let target = Path::new("/Volumes/x/work");
        let copy = prepare_import(&projects, &source, target, SAMPLE_ID, ImportMode::Copy)
            .expect("should succeed")
            .expect("a copy should be created");

        assert_eq!(
            copy,
            projects
                .join("-Volumes-x-work")
                .join(format!("{SAMPLE_ID}.jsonl"))
        );
        // A real file, not a symlink pointing back into the other user's tree.
        assert!(
            !fs::symlink_metadata(&copy)
                .unwrap()
                .file_type()
                .is_symlink(),
            "cross-user import must be a copy, not a symlink"
        );
        assert_eq!(
            fs::read_to_string(&copy).unwrap(),
            "{\"cwd\":\"/work\"}\n",
            "the copy must snapshot the source bytes"
        );

        // A second import finds the file already present and is a no-op.
        assert_eq!(
            prepare_import(&projects, &source, target, SAMPLE_ID, ImportMode::Copy).expect("ok"),
            None
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
    fn user_projects_sets_is_self_for_self_and_other() {
        let parent = Path::new("/Users");
        // A `--user` naming the current account is a same-user hit.
        let me = user_projects(parent, "timmattison", "timmattison");
        assert_eq!(me.user, "timmattison");
        assert_eq!(
            me.projects_dir,
            Path::new("/Users/timmattison/.claude/projects")
        );
        assert!(me.is_self);

        // A different account is a cross-user root.
        let other = user_projects(parent, "scyloswork", "timmattison");
        assert_eq!(other.user, "scyloswork");
        assert_eq!(
            other.projects_dir,
            Path::new("/Users/scyloswork/.claude/projects")
        );
        assert!(!other.is_self);
    }

    #[test]
    fn self_projects_marks_the_current_users_tree() {
        let home = Path::new("/Users/timmattison");
        let mine = self_projects(home);
        assert_eq!(mine.user, "timmattison");
        assert_eq!(
            mine.projects_dir,
            Path::new("/Users/timmattison/.claude/projects")
        );
        assert!(mine.is_self);
    }

    #[test]
    fn find_session_across_finds_id_in_a_single_root() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("home/.claude/projects");
        let proj = projects.join("-some-proj");
        fs::create_dir_all(&proj).unwrap();
        let file = proj.join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{}\n").unwrap();

        let root = UserProjects {
            user: "me".to_string(),
            projects_dir: projects,
            is_self: true,
        };
        match find_session_across(std::slice::from_ref(&root), SAMPLE_ID) {
            FoundSession::Found { path, root: found } => {
                assert_eq!(path, file);
                assert_eq!(found.user, "me");
                assert!(found.is_self);
            }
            FoundSession::NotFound => panic!("expected the id to be found"),
        }
    }

    #[test]
    fn find_session_across_reports_not_found_when_absent() {
        let dir = tempdir().unwrap();
        let projects = dir.path().join("home/.claude/projects");
        fs::create_dir_all(&projects).unwrap();
        let root = UserProjects {
            user: "me".to_string(),
            projects_dir: projects,
            is_self: true,
        };
        assert!(matches!(
            find_session_across(&[root], SAMPLE_ID),
            FoundSession::NotFound
        ));
    }

    #[test]
    fn session_dir_from_transcript_errors_when_unreadable() {
        let dir = tempdir().unwrap();
        // A transcript that cannot be read resolves to SessionNotFound.
        let missing = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        assert!(matches!(
            session_dir_from_transcript(&missing),
            Err(ResolveError::SessionNotFound)
        ));
    }

    #[test]
    fn session_dir_from_transcript_errors_when_no_cwd() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        fs::write(&file, "{\"cwd\":null}\n").unwrap();

        assert!(matches!(
            session_dir_from_transcript(&file),
            Err(ResolveError::NoCwdInSession)
        ));
    }

    #[test]
    fn session_dir_from_transcript_errors_when_directory_missing() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        let missing = dir.path().join("gone");
        fs::write(&file, cwd_line(missing.to_str().unwrap())).unwrap();

        match session_dir_from_transcript(&file) {
            Err(ResolveError::DirectoryMissing(path)) => assert_eq!(path, missing),
            other => panic!("expected DirectoryMissing, got {other:?}"),
        }
    }

    #[test]
    fn session_dir_from_transcript_returns_existing_directory() {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!("{SAMPLE_ID}.jsonl"));
        let cwd = dir.path().join("real-cwd");
        fs::create_dir_all(&cwd).unwrap();
        fs::write(&file, cwd_line(cwd.to_str().unwrap())).unwrap();

        assert_eq!(session_dir_from_transcript(&file).unwrap(), cwd);
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

    /// The session id recorded in [`SESSION_JSON`].
    const LIVE_ID: &str = "3eafa9f8-9d1f-43cf-b417-eb9efcb8ed4d";

    /// Writes a transcript whose single assistant turn ended cleanly (so the
    /// classifier reports `waiting-for-user`) into a project subfolder.
    fn write_waiting_transcript(projects: &Path, session_id: &str) {
        let proj = projects.join("proj");
        fs::create_dir_all(&proj).unwrap();
        fs::write(
            proj.join(format!("{session_id}.jsonl")),
            assistant_line(block("text"), serde_json::json!("end_turn")),
        )
        .unwrap();
    }

    #[test]
    fn resolve_status_report_prefers_live_session_over_transcript() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        // A transcript that would classify as waiting-for-user...
        write_waiting_transcript(projects.path(), LIVE_ID);
        // ...but the session is live, so the live status wins for `state`.
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let report = resolve_status_report(projects.path(), sessions.path(), LIVE_ID, |pid| {
            pid == 17041
        })
        .unwrap();
        assert_eq!(report.state, "busy (live, pid 17041)");
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
    fn shell_code_removes_here_symlink_early_via_background_watcher() {
        // A backgrounded watcher polls the project folder and removes the
        // symlink as soon as a new (forked) session file appears — Claude no
        // longer needs the symlink once it has read the transcript — instead of
        // letting it linger for the whole session.
        assert!(SHELL_CODE.contains(r#"find "$__crap_folder""#));
        assert!(SHELL_CODE.contains(r#"-gt "$__crap_n0""#));
        assert!(SHELL_CODE.contains(") &"));
        assert!(SHELL_CODE.contains("sleep 0.1"));
        // The watcher is stopped once claude exits, and the post-exit `rm`
        // remains as a safety net in case the fork file never appeared.
        assert!(SHELL_CODE.contains(r#"kill "$__crap_watcher""#));
    }

    #[test]
    fn here_output_carries_sentinel_session_and_link() {
        let link = Path::new("/Users/tim/.claude/projects/-x/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, None, Some(link));

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(HERE_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        // No forced id was given, so the third field is the sentinel.
        assert_eq!(lines.next(), Some(NO_NEW_ID_SENTINEL));
        // Everything after the third newline is the link path, intact.
        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn here_output_uses_no_link_sentinel_when_nothing_to_clean() {
        let out = format_here_output(SAMPLE_ID, None, None);

        assert_eq!(out.lines().next(), Some(HERE_SENTINEL));
        let link_field = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(link_field.trim_end_matches('\n'), NO_LINK_SENTINEL);
    }

    #[test]
    fn here_output_carries_forced_new_session_id() {
        // When the caller supplies a forked-session id, it rides as the third
        // field so the shell function can pass it to `claude --session-id`.
        let link = Path::new("/Users/tim/.claude/projects/-x/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, Some(ID_B), Some(link));

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(HERE_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        assert_eq!(lines.next(), Some(ID_B));
        // The link still lives last, after the forced-id field.
        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn here_output_uses_no_new_id_sentinel_when_absent() {
        // Without a caller-supplied id, the third field is the sentinel so the
        // shell function lets Claude mint a fresh random id.
        let out = format_here_output(SAMPLE_ID, None, None);
        assert_eq!(out.lines().nth(2), Some(NO_NEW_ID_SENTINEL));
    }

    #[test]
    fn here_output_preserves_newline_in_link_path() {
        // The link lives last in the output, so a newline inside the path can't
        // be mistaken for a field boundary.
        let link = Path::new("/Users/tim/od\ndd/abc.jsonl");
        let out = format_here_output(SAMPLE_ID, None, Some(link));

        let rest = out.splitn(4, '\n').nth(3).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), link.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_emits_dir_last_with_sentinels_in_slots() {
        // Cross-user default resume: sentinel, session, new-id slot, link slot,
        // then the original directory last.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, None, Some(link), dir);

        let mut lines = out.lines();
        assert_eq!(lines.next(), Some(FORK_AT_SENTINEL));
        assert_eq!(lines.next(), Some(SAMPLE_ID));
        // No forced id: the new-id slot is the sentinel.
        assert_eq!(lines.next(), Some(NO_NEW_ID_SENTINEL));
        assert_eq!(lines.next(), Some(link.to_str().unwrap()));
        // Everything after the fourth newline is the directory, intact.
        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_uses_no_link_sentinel_when_nothing_to_clean() {
        // When the import was a no-op (already resolvable), the link slot is the
        // sentinel so the shell knows there is nothing to remove.
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, None, None, dir);
        assert_eq!(out.lines().nth(3), Some(NO_LINK_SENTINEL));
    }

    #[test]
    fn fork_at_output_carries_forced_new_id() {
        // A caller-supplied forked id rides in the new-id slot for the shell's
        // `--session-id`, while the directory still comes last.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/work");
        let out = format_fork_at_output(SAMPLE_ID, Some(ID_B), Some(link), dir);
        assert_eq!(out.lines().nth(2), Some(ID_B));
        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
    }

    #[test]
    fn fork_at_output_preserves_newline_in_dir() {
        // The directory lives last, so a newline inside it can't be mistaken for
        // a field boundary — the invariant the whole layout is designed around.
        let link = Path::new("/Users/tim/.claude/projects/-work/abc.jsonl");
        let dir = Path::new("/Volumes/x/od\ndd");
        let out = format_fork_at_output(SAMPLE_ID, None, Some(link), dir);

        let rest = out.splitn(5, '\n').nth(4).unwrap();
        assert_eq!(rest.trim_end_matches('\n'), dir.to_str().unwrap());
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

    #[test]
    fn shell_code_passes_informational_flags_through_untouched() {
        // `--status` queries, --help/-h/--version/-V print informational text,
        // and --shell-setup writes the rc file (not the live shell); none mutate
        // the parent shell, and each must reach the terminal rather than being
        // parsed as a "<session-id>\n<dir>" resume target.
        assert!(SHELL_CODE.contains(
            r#"*" --status "*|*" --help "*|*" -h "*|*" --version "*|*" -V "*|*" --shell-setup "*)"#
        ));
        assert!(SHELL_CODE.contains(r#"command crap "$@"; return $?"#));
    }

    /// Sources `SHELL_CODE` in a real `bash`, with a fake `crap` binary (and
    /// fake `claude`/`clauded`) ahead of it on `PATH`, then runs `crap <args>`.
    ///
    /// The fake binary mimics clap: informational flags print a recognizable
    /// marker to stdout and exit 0; anything else emits a `<session>\n<dir>`
    /// resume target. Returns the captured stdout plus whether the `claude`
    /// stub was invoked (it drops a marker file when called).
    ///
    /// Each call gets its own `tempfile::TempDir` (an `O_EXCL` random name) so
    /// concurrent runs of this test never share a directory. A pid+nanos name is
    /// NOT enough: two threads in the same test process can sample the clock in
    /// the same tick and collide, letting one run clobber the other's fakes.
    #[cfg(unix)]
    fn run_shell_function(args: &str) -> (String, bool) {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let claude_marker = dir.join("claude_called");

        // Fake `crap`: informational flags print a marker and exit 0, exactly
        // as clap does; the default path prints a resume target.
        let fake_crap = "#!/bin/sh\n\
            case \" $* \" in\n\
            \x20 *\" --help \"*|*\" -h \"*) printf 'CRAP_HELP_MARKER\\nUsage: crap\\nmore\\n'; exit 0 ;;\n\
            \x20 *\" --version \"*|*\" -V \"*) printf 'CRAP_VERSION_MARKER 0.1.0\\n'; exit 0 ;;\n\
            \x20 *\" --shell-setup \"*) printf 'CRAP_SETUP_MARKER\\nTo activate, run:\\n  source ~/.zshrc\\n'; exit 0 ;;\n\
            esac\n\
            printf 'session-xyz\\n/tmp/crap-resume-dir\\n'\n";

        // Fake `claude`/`clauded`: record that a resume was attempted.
        let fake_claude = format!("#!/bin/sh\n: > {:?}\n", claude_marker);

        for (name, body) in [
            ("crap", fake_crap.to_string()),
            ("claude", fake_claude.clone()),
            ("clauded", fake_claude),
        ] {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap {args}\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let claude_called = claude_marker.exists();
        // `temp` drops at end of scope, removing the directory.
        (stdout, claude_called)
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_passes_informational_flags_through() {
        // These flags make the binary print to stdout and exit 0 without
        // mutating the parent shell. Without a pass-through, the function
        // captures that text and tries to `cd` into it as a resume directory.
        // Each must reach the terminal verbatim and never trigger a resume.
        // `--shell-setup` is included because, on an upgrade, the already-loaded
        // function would otherwise swallow its activation instructions.
        for (args, marker) in [
            ("--help", "CRAP_HELP_MARKER"),
            ("-h", "CRAP_HELP_MARKER"),
            ("--version", "CRAP_VERSION_MARKER"),
            ("-V", "CRAP_VERSION_MARKER"),
            ("--shell-setup", "CRAP_SETUP_MARKER"),
        ] {
            let (stdout, claude_called) = run_shell_function(args);
            assert!(
                stdout.contains(marker),
                "`crap {args}` should print the binary's output verbatim, got: {stdout:?}"
            );
            assert!(!claude_called, "`crap {args}` must not attempt a resume");
        }
    }

    /// Sources `SHELL_CODE` in a real `bash` with a fake `crap` that emits
    /// here-mode output carrying `new_id_field` as its third field (and
    /// `__CRAP_NO_LINK__`, so the symlink watcher is skipped), plus fake
    /// `claude`/`clauded` that record the exact arguments they were resumed
    /// with. Returns those recorded arguments, one per line.
    ///
    /// When `provide_clauded` is true the preferred `clauded` is on `PATH` and
    /// records; otherwise only plain `claude` is available. Each call gets its
    /// own `tempfile::TempDir` (an `O_EXCL` random name) so concurrent runs
    /// never share a directory. A pid+nanos name is NOT enough: two threads in
    /// the same test process can sample the clock in the same tick and collide,
    /// letting one run read args the other recorded.
    #[cfg(unix)]
    fn run_here_shell_function(new_id_field: &str, provide_clauded: bool) -> String {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        // The session id the fake binary reports as the resumed original.
        const HERE_SESSION: &str = "33333333-4444-5555-6666-777777777777";

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let args_file = dir.join("claude_args");

        // Fake `crap`: emit a here-output whose third field is `new_id_field`.
        let fake_crap = format!(
            "#!/bin/sh\nprintf '{HERE_SENTINEL}\\n%s\\n%s\\n%s\\n' '{HERE_SESSION}' '{new_id_field}' '{NO_LINK_SENTINEL}'\n"
        );

        // Fake `claude`/`clauded`: record the exact argument list, one per line.
        let fake_claude = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {:?}\n", args_file);

        let mut tools = vec![("crap", fake_crap), ("claude", fake_claude.clone())];
        if provide_clauded {
            tools.push(("clauded", fake_claude));
        }
        for (name, body) in tools {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap --here {HERE_SESSION}\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");
        assert!(
            output.status.success(),
            "here-mode shell function failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let recorded = fs::read_to_string(&args_file).unwrap_or_default();
        // `temp` drops at end of scope, removing the directory.
        recorded
    }

    /// A well-formed forked-session id for the here-mode dispatch tests.
    const FORCED_NEW_ID: &str = "99999999-8888-7777-6666-555555555555";

    #[cfg(unix)]
    #[test]
    fn shell_function_pins_forced_new_id_via_session_id() {
        // When the binary supplies a forced id, the resume must fork *and* pin
        // the fork to that id with `--session-id`, on both the `clauded` and the
        // plain `claude` dispatch paths.
        for provide_clauded in [true, false] {
            let recorded = run_here_shell_function(FORCED_NEW_ID, provide_clauded);
            let args: Vec<&str> = recorded.lines().collect();
            assert!(
                args.contains(&"--fork-session"),
                "must still fork (clauded={provide_clauded}); got {args:?}"
            );
            let pos = args
                .iter()
                .position(|a| *a == "--session-id")
                .unwrap_or_else(|| {
                    panic!("--session-id missing (clauded={provide_clauded}): {args:?}")
                });
            assert_eq!(
                args.get(pos + 1).copied(),
                Some(FORCED_NEW_ID),
                "the forced id must follow --session-id (clauded={provide_clauded})"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_omits_session_id_without_a_forced_new_id() {
        // The sentinel third field means "let Claude mint a random id": the
        // resume forks but must not pass --session-id.
        let recorded = run_here_shell_function(NO_NEW_ID_SENTINEL, true);
        let args: Vec<&str> = recorded.lines().collect();
        assert!(
            args.contains(&"--fork-session"),
            "must still fork: {args:?}"
        );
        assert!(
            !args.contains(&"--session-id"),
            "no forced id => no --session-id: {args:?}"
        );
    }

    #[test]
    fn shell_code_parses_new_id_field_and_pins_session_id() {
        // here-mode reads the third field and, unless it is the sentinel, pins
        // the fork's id via `claude --session-id`.
        assert!(SHELL_CODE.contains(NO_NEW_ID_SENTINEL));
        assert!(SHELL_CODE.contains("--session-id"));
    }

    #[test]
    fn shell_code_detects_fork_at_sentinel() {
        // The shell function branches on the exact cross-user sentinel the
        // binary emits for a resume at the session's original directory.
        assert!(SHELL_CODE.contains(FORK_AT_SENTINEL));
    }

    /// Sources `SHELL_CODE` in a real `bash` with a fake `crap` that emits
    /// cross-user `__CRAP_FORK_AT__` output naming a real `orig-cwd` directory
    /// (and `__CRAP_NO_LINK__`, so no watcher/cleanup runs), plus a fake
    /// `claude`/`clauded` that records both the arguments it was resumed with
    /// and the working directory it ran in. Returns `(recorded_args,
    /// resumed_in_dir, orig_dir)`, where both directories are canonicalized
    /// **before** the temp dir is dropped so the caller can compare them without
    /// touching a filesystem path that no longer exists.
    ///
    /// Each call gets its own `tempfile::TempDir` (an `O_EXCL` random name) so
    /// concurrent runs never share a directory.
    #[cfg(unix)]
    fn run_fork_at_shell_function() -> (String, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        // The session id the fake binary reports as the foreign original.
        const FORK_SESSION: &str = "33333333-4444-5555-6666-777777777777";

        let temp = tempfile::TempDir::new().unwrap();
        let dir = temp.path();

        let args_file = dir.join("claude_args");
        let pwd_file = dir.join("claude_pwd");
        // The session's original recorded directory: the fork must land here.
        let orig = dir.join("orig-cwd");
        fs::create_dir_all(&orig).unwrap();

        // Fake `crap`: emit a fork-at output naming `orig` as the last field.
        let fake_crap = format!(
            "#!/bin/sh\nprintf '{FORK_AT_SENTINEL}\\n%s\\n%s\\n%s\\n%s\\n' '{FORK_SESSION}' '{NO_NEW_ID_SENTINEL}' '{NO_LINK_SENTINEL}' '{}'\n",
            orig.display()
        );
        // Fake `claude`/`clauded`: record the resume argv and the cwd it ran in.
        let fake_claude = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > {:?}\npwd > {:?}\n",
            args_file, pwd_file
        );

        for (name, body) in [
            ("crap", fake_crap),
            ("claude", fake_claude.clone()),
            ("clauded", fake_claude),
        ] {
            let path = dir.join(name);
            fs::write(&path, body).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let base_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{base_path}", dir.display());
        let script = format!("{SHELL_CODE}\ncrap {FORK_SESSION} --user someone\n");

        let output = Command::new("bash")
            .env("PATH", new_path)
            .arg("-c")
            .arg(&script)
            .output()
            .expect("bash should be available");

        let args = fs::read_to_string(&args_file).unwrap_or_default();
        let pwd = fs::read_to_string(&pwd_file).unwrap_or_default();
        assert!(
            output.status.success(),
            "fork-at shell function failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Canonicalize both directories while `temp` is still alive — it drops
        // (removing the tree) at the end of this scope, so the caller must not
        // depend on either path still existing on disk.
        let resumed_in = std::fs::canonicalize(pwd.trim())
            .expect("claude should have recorded the directory it forked in");
        let orig = std::fs::canonicalize(&orig).unwrap();
        (args, resumed_in, orig)
    }

    #[cfg(unix)]
    #[test]
    fn shell_function_forks_at_original_dir_for_cross_user() {
        // Cross-user default resume: the function must `cd` into the session's
        // original directory and then fork it (`--resume <id> --fork-session`),
        // leaving the foreign transcript untouched.
        let (args, resumed_in, orig) = run_fork_at_shell_function();
        let args: Vec<&str> = args.lines().collect();
        assert!(
            args.contains(&"--resume") && args.contains(&"--fork-session"),
            "must fork-resume the original id; got {args:?}"
        );
        assert_eq!(
            resumed_in, orig,
            "the fork must run in the session's original directory"
        );
    }

    // Two distinct session ids for the per-directory listing tests.
    const ID_A: &str = "aaaaaaaa-1111-2222-3333-444444444444";
    const ID_B: &str = "bbbbbbbb-1111-2222-3333-444444444444";

    /// Builds a transcript line of `kind` carrying a top-level `timestamp`.
    fn timestamped_line(kind: &str, timestamp: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({ "type": kind, "timestamp": timestamp, "message": {} })
        )
    }

    #[test]
    fn transcript_time_span_returns_earliest_and_latest() {
        let contents = format!(
            "{}{}{}",
            timestamped_line("user", "2026-05-25T18:43:05.109Z"),
            timestamped_line("assistant", "2026-05-25T19:00:00.000Z"),
            timestamped_line("assistant", "2026-05-25T20:17:39.732Z"),
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T18:43:05.109Z".to_string()),
                Some("2026-05-25T20:17:39.732Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_ignores_lines_without_timestamps() {
        // Bookkeeping lines carry no timestamp and must not affect the span.
        let contents = format!(
            "{}{}{}",
            "{\"type\":\"last-prompt\"}\n",
            timestamped_line("user", "2026-05-25T18:43:05.109Z"),
            "{\"type\":\"ai-title\"}\n",
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T18:43:05.109Z".to_string()),
                Some("2026-05-25T18:43:05.109Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_is_order_independent() {
        // A line written later may bear an earlier instant; min/max still hold.
        let contents = format!(
            "{}{}",
            timestamped_line("assistant", "2026-05-25T20:00:00.000Z"),
            timestamped_line("user", "2026-05-25T08:00:00.000Z"),
        );
        assert_eq!(
            transcript_time_span(&contents),
            (
                Some("2026-05-25T08:00:00.000Z".to_string()),
                Some("2026-05-25T20:00:00.000Z".to_string())
            )
        );
    }

    #[test]
    fn transcript_time_span_none_when_no_timestamps() {
        assert_eq!(
            transcript_time_span("{\"type\":\"last-prompt\"}\n"),
            (None, None)
        );
    }

    #[test]
    fn format_timestamp_prettifies_iso8601() {
        assert_eq!(
            format_timestamp("2026-05-25T18:43:05.109Z"),
            "2026-05-25 18:43:05"
        );
    }

    #[test]
    fn format_timestamp_handles_missing_subseconds() {
        assert_eq!(
            format_timestamp("2026-05-25T18:43:05Z"),
            "2026-05-25 18:43:05"
        );
    }

    #[test]
    fn format_timestamp_passes_through_unexpected_input() {
        assert_eq!(format_timestamp("not-a-timestamp"), "not-a-timestamp");
    }

    /// Writes a single-turn transcript (classifies as `waiting-for-user`) with
    /// one `timestamp` into `folder`.
    fn write_session_in(folder: &Path, session_id: &str, timestamp: &str) {
        fs::create_dir_all(folder).unwrap();
        let line = format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "timestamp": timestamp,
                "message": { "stop_reason": "end_turn", "content": [{ "type": "text" }] },
            })
        );
        fs::write(folder.join(format!("{session_id}.jsonl")), line).unwrap();
    }

    #[test]
    fn resolve_dir_statuses_lists_all_sessions_in_pwd_folder() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let pwd = Path::new("/Volumes/x/proj");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, ID_A, "2026-05-25T10:00:00.000Z");
        write_session_in(&folder, ID_B, "2026-05-25T11:00:00.000Z");

        let reports = resolve_dir_statuses(projects.path(), sessions.path(), pwd, |_| false);
        assert_eq!(reports.len(), 2);
        // Ascending by last-activity: the most recently used session is last,
        // so it lands at the bottom of the printed table.
        assert_eq!(reports[0].session_id, ID_A);
        assert_eq!(reports[1].session_id, ID_B);
        assert!(reports.iter().all(|r| r.state == "waiting-for-user"));
        assert_eq!(
            reports[1].started.as_deref(),
            Some("2026-05-25T11:00:00.000Z")
        );
        assert_eq!(reports[1].last.as_deref(), Some("2026-05-25T11:00:00.000Z"));
    }

    #[test]
    fn resolve_dir_statuses_ignores_non_session_files() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let pwd = Path::new("/Volumes/x/proj");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, ID_A, "2026-05-25T10:00:00.000Z");
        fs::write(folder.join("notes.txt"), "hi").unwrap();
        fs::write(folder.join("not-a-uuid.jsonl"), "{}\n").unwrap();

        let reports = resolve_dir_statuses(projects.path(), sessions.path(), pwd, |_| false);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].session_id, ID_A);
    }

    #[test]
    fn resolve_dir_statuses_empty_when_folder_absent() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let reports = resolve_dir_statuses(
            projects.path(),
            sessions.path(),
            Path::new("/no/such/dir"),
            |_| false,
        );
        assert!(reports.is_empty());
    }

    #[test]
    fn resolve_dir_statuses_marks_live_session() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        // The cwd's project folder holds the live session's transcript, but the
        // live process's own status wins over transcript inference.
        let pwd = Path::new("/Volumes/code/crap");
        let folder = projects.path().join(encode_project_dir(pwd));
        write_session_in(&folder, LIVE_ID, "2026-05-25T10:00:00.000Z");
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let reports =
            resolve_dir_statuses(projects.path(), sessions.path(), pwd, |pid| pid == 17041);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].state, "busy (live, pid 17041)");
    }

    #[test]
    fn format_dir_statuses_reports_none_found_for_empty() {
        let out = format_dir_statuses(Path::new("/Volumes/x/proj"), &[]);
        assert!(out.contains("No Claude sessions found"));
        assert!(out.contains("/Volumes/x/proj"));
    }

    #[test]
    fn format_dir_statuses_tabulates_each_session_with_times() {
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let out = format_dir_statuses(Path::new("/Volumes/x/proj"), &reports);
        // A heading line, then a table with column headers.
        assert!(out.contains("1 session for /Volumes/x/proj"));
        assert!(out.contains("SESSION"));
        assert!(out.contains("STATE"));
        assert!(out.contains("STARTED"));
        assert!(out.contains("LAST"));
        assert!(out.contains(ID_A));
        assert!(out.contains("waiting-for-user"));
        // Times are prettified in the cells (no raw `T`/`Z`, no `started`/`last`
        // labels — those are column headers now).
        assert!(out.contains("2026-05-25 10:00:00"));
        assert!(out.contains("2026-05-25 12:30:45"));
    }

    #[test]
    fn dir_statuses_table_never_wraps_cells_to_fit_a_narrow_terminal() {
        // The status table must render at its natural content width regardless of
        // the terminal: a session UUID or timestamp chopped across lines is
        // unreadable, and tying the layout to the ambient terminal width makes the
        // output (and every test that asserts on it) depend on a shared,
        // uncontrolled resource — a flaky-test trap. Force an absurdly narrow
        // width and demand every cell still appears whole on one line.
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let mut table = dir_statuses_table(&reports);
        table.set_width(20);
        let rendered = table.to_string();
        assert!(
            rendered.contains(ID_A),
            "UUID must stay intact at narrow width, got:\n{rendered}"
        );
        assert!(rendered.contains("waiting-for-user"));
        assert!(rendered.contains("2026-05-25 10:00:00"));
        assert!(rendered.contains("2026-05-25 12:30:45"));
    }

    #[test]
    fn format_dir_statuses_uses_plural_and_dash_for_missing_times() {
        let reports = vec![
            SessionStatusReport {
                session_id: ID_A.to_string(),
                state: "empty".to_string(),
                started: None,
                last: None,
            },
            SessionStatusReport {
                session_id: ID_B.to_string(),
                state: "busy".to_string(),
                started: Some("2026-05-25T10:00:00.000Z".to_string()),
                last: Some("2026-05-25T10:05:00.000Z".to_string()),
            },
        ];
        let out = format_dir_statuses(Path::new("/x"), &reports);
        assert!(out.contains("2 sessions for /x"));
        // The session with no recorded activity shows an em dash placeholder.
        assert!(out.contains("—"));
    }

    #[test]
    fn format_dir_statuses_json_emits_array_with_raw_timestamps() {
        let reports = vec![SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "busy (live, pid 17041)".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: Some("2026-05-25T12:30:45.000Z".to_string()),
        }];
        let out = format_dir_statuses_json(&reports);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let arr = parsed.as_array().expect("a JSON array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["sessionId"], ID_A);
        assert_eq!(arr[0]["state"], "busy (live, pid 17041)");
        // Raw ISO 8601 is preserved for machine consumers, not prettified.
        assert_eq!(arr[0]["started"], "2026-05-25T10:00:00.000Z");
        assert_eq!(arr[0]["last"], "2026-05-25T12:30:45.000Z");
    }

    #[test]
    fn format_dir_statuses_json_empty_is_empty_array() {
        let parsed: serde_json::Value =
            serde_json::from_str(&format_dir_statuses_json(&[])).expect("valid JSON");
        assert_eq!(parsed.as_array().expect("a JSON array").len(), 0);
    }

    #[test]
    fn format_status_json_emits_single_object() {
        let report = SessionStatusReport {
            session_id: ID_A.to_string(),
            state: "waiting-for-user".to_string(),
            started: Some("2026-05-25T10:00:00.000Z".to_string()),
            last: None,
        };
        let parsed: serde_json::Value =
            serde_json::from_str(&format_status_json(&report)).expect("valid JSON");
        assert_eq!(parsed["sessionId"], ID_A);
        assert_eq!(parsed["state"], "waiting-for-user");
        assert_eq!(parsed["started"], "2026-05-25T10:00:00.000Z");
        assert!(parsed["last"].is_null());
    }

    #[test]
    fn resolve_status_report_carries_state_and_time_span() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        let proj = projects.path().join("proj");
        fs::create_dir_all(&proj).unwrap();
        let line = format!(
            "{}\n",
            serde_json::json!({
                "type": "assistant",
                "timestamp": "2026-05-25T10:00:00.000Z",
                "message": { "stop_reason": "end_turn", "content": [{ "type": "text" }] },
            })
        );
        fs::write(proj.join(format!("{SAMPLE_ID}.jsonl")), line).unwrap();

        let report =
            resolve_status_report(projects.path(), sessions.path(), SAMPLE_ID, |_| false).unwrap();
        assert_eq!(report.session_id, SAMPLE_ID);
        assert_eq!(report.state, "waiting-for-user");
        assert_eq!(report.started.as_deref(), Some("2026-05-25T10:00:00.000Z"));
        assert_eq!(report.last.as_deref(), Some("2026-05-25T10:00:00.000Z"));
    }

    #[test]
    fn resolve_status_report_uses_live_state_for_the_state_field() {
        let projects = tempdir().unwrap();
        let sessions = tempdir().unwrap();
        fs::write(sessions.path().join("17041.json"), SESSION_JSON).unwrap();

        let report = resolve_status_report(projects.path(), sessions.path(), LIVE_ID, |pid| {
            pid == 17041
        })
        .unwrap();
        assert_eq!(report.state, "busy (live, pid 17041)");
    }

    #[test]
    fn resolve_status_report_rejects_invalid_id() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_status_report(dir.path(), dir.path(), "../escape", |_| true),
            Err(StatusError::InvalidSessionId)
        ));
    }

    #[test]
    fn resolve_status_report_errors_when_session_missing() {
        let dir = tempdir().unwrap();
        assert!(matches!(
            resolve_status_report(dir.path(), dir.path(), SAMPLE_ID, |_| false),
            Err(StatusError::SessionNotFound)
        ));
    }

    #[test]
    fn cli_json_requires_status() {
        use clap::Parser;
        // --json without --status is rejected.
        assert!(Cli::try_parse_from(["crap", "--json", SAMPLE_ID]).is_err());
    }

    #[test]
    fn cli_status_json_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["crap", "--status", "--json"]).expect("should parse");
        assert!(cli.status && cli.json);
    }

    #[test]
    fn cli_allows_status_without_session_id() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["crap", "--status"]).expect("--status with no id should parse");
        assert!(cli.status);
        assert!(cli.session_id.is_none());
    }

    #[test]
    fn cli_still_requires_session_id_without_flags() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["crap"]).is_err());
    }
}
