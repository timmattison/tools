//! `tsm` - terminal session manager.
//!
//! This binary is the entry point for the tsm tool. It currently provides
//! three subcommands:
//!
//! - `tsm --version` prints `tsm <version> (<short-hash>, clean|dirty)`.
//! - `tsm shell-init <shell>` emits an eval-able shell snippet that wires up
//!   a per-command precmd hook calling `tsm record` in the background.
//! - `tsm record` is the precmd-time recorder; in this slice it is a stub
//!   that accepts the same args the snippet will pass.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcCommand, Stdio};
use std::time::Duration;

use buildinfo::version_string;
use chrono::Utc;
use clap::{Parser, Subcommand};
use tsm_id::{SessionId, session_id_from_tuple};
use tsm_jsonl::{
    Header, HeaderKind, JsonlError, PrecmdKind, PrecmdRecord, TupleStub, append_header,
    append_record,
};
use tsm_tuple::{Env as TupleEnv, LayoutText, Tuple, TupleError, derive_tuple};
use wait_timeout::ChildExt;

/// Exit code returned when `tsm shell-init` receives an unsupported shell.
const EXIT_UNSUPPORTED_SHELL: i32 = 2;

/// Watchdog timeout for any subprocess the recorder spawns.
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(2);

/// Soft upper bound on the error log size in bytes; rotation kicks in above this.
const ERROR_LOG_MAX_BYTES: u64 = 1 << 20; // 1 MiB

/// Bytes kept after rotation.
const ERROR_LOG_KEEP_BYTES: usize = 1 << 19; // 512 KiB

/// Hardcoded redaction patterns. Three shapes are supported:
/// - exact match (full uppercase name)
/// - suffix match (`*_FOO` — `_FOO` literal suffix)
/// - prefix match (`FOO_*` — `FOO_` literal prefix)
///
/// Matching is case-insensitive on the env var name (we uppercase once).
const REDACTION_SUFFIXES: &[&str] = &["_TOKEN", "_KEY", "_SECRET", "_PASSWORD", "_PASSWD"];
const REDACTION_PREFIXES: &[&str] = &["AWS_", "OP_", "CLAUDE_"];
const REDACTION_EXACT: &[&str] = &[
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
];

/// Top-level CLI for `tsm`.
#[derive(Parser)]
#[command(name = "tsm")]
#[command(about = "Terminal session manager")]
#[command(version = version_string!())]
struct Cli {
    /// The subcommand to invoke.
    #[command(subcommand)]
    command: Commands,
}

/// All `tsm` subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Emit an eval-able shell snippet that wires up the precmd hook.
    ShellInit {
        /// The shell to emit integration for. Only `zsh` is supported in v1.
        shell: String,
    },
    /// Print a diagnostic report about the Zellij environment and derived
    /// session id. Exit code is always 0; failures are part of the report.
    Doctor,
    /// Record one command's metadata. Invoked by the shell precmd hook.
    Record {
        /// Exit status of the last command in the shell.
        #[arg(long, allow_hyphen_values = true, default_value_t = 0)]
        exit_code: i32,

        /// Verbatim text of the last command line.
        #[arg(long, default_value = "")]
        last_command: String,

        /// Hidden: synthetic subprocess probe for the watchdog acceptance test.
        ///
        /// When set, the recorder skips its normal append path and instead
        /// spawns the provided command (split on whitespace, no shell), wraps
        /// it in the watchdog timeout, and exits 0 regardless of outcome.
        #[arg(long, hide = true)]
        probe_subprocess: Option<String>,
    },
}

/// Return true if `name` matches any of the hardcoded redaction patterns.
///
/// Matching is case-insensitive on the env var name. The value is never
/// inspected — pattern decisions are purely structural on the key.
fn is_redacted(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if REDACTION_EXACT.iter().any(|p| upper == *p) {
        return true;
    }
    if REDACTION_PREFIXES.iter().any(|p| upper.starts_with(p)) {
        return true;
    }
    if REDACTION_SUFFIXES.iter().any(|p| upper.ends_with(p)) {
        return true;
    }
    false
}

/// Split `env` into (kept, redacted-keys-sorted). Values for keys that match
/// the redaction list are dropped entirely from the kept map; their names are
/// pushed onto `redacted_keys` and the result is sorted alphabetically.
fn redact_env(
    env: BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<String>) {
    let mut kept = BTreeMap::new();
    let mut redacted = Vec::new();
    for (k, v) in env {
        if is_redacted(&k) {
            redacted.push(k);
        } else {
            kept.insert(k, v);
        }
    }
    redacted.sort();
    (kept, redacted)
}

/// Maximum number of consecutive `tsm record` failures before the precmd
/// hook self-disables for the current shell.
const FAILURE_THRESHOLD: u32 = 3;

/// Generate the zsh `eval`-able shell-init snippet.
///
/// The emitted snippet, when sourced via `eval "$(tsm shell-init zsh)"`:
///
/// 1. Bails out silently if `$TSM_DISABLE` is set in the environment.
/// 2. Runs `tsm --version` as a health check with a ~1 second timeout. On
///    failure it prints one warning to stderr and returns without installing
///    the hook.
/// 3. Exports `$TSM_SESSION_ID`. The id is inlined into the snippet at print
///    time (one fresh id per `tsm shell-init zsh` invocation). If the env var
///    is already set on entry, the existing value is preserved.
/// 4. Registers a `precmd` hook that backgrounds `tsm record &!`, passing
///    `--exit-code` and `--last-command`. Before backgrounding, the hook
///    reads `$XDG_STATE_HOME/tsm/fail-count.<shell-pid>`; if the count has
///    reached the failure threshold it unregisters itself and emits one
///    warning line pointing at the error log.
/// 5. Does NOT emit any OSC pane-title sequences; pane-title work is deferred
///    to a later slice.
fn generate_zsh_snippet(session_id: &SessionId) -> String {
    let id = session_id.as_hex();
    let threshold = FAILURE_THRESHOLD;
    format!(
        r#"# === tsm shell integration (begin) ===
if [[ -n "${{TSM_DISABLE-}}" ]]; then
    return 0 2>/dev/null || exit 0
fi

# Health check: tsm --version with a ~1 second timeout. macOS does not ship
# GNU timeout(1), so we implement the timeout inline by backgrounding the
# health check, racing it against a sleep, and killing the loser.
_tsm_health_check() {{
    local _pid _watchdog _rc
    ( tsm --version >/dev/null 2>&1 ) &
    _pid=$!
    ( sleep 1 && kill -TERM $_pid 2>/dev/null ) &!
    _watchdog=$!
    if wait $_pid 2>/dev/null; then
        _rc=0
    else
        _rc=$?
    fi
    kill $_watchdog 2>/dev/null
    return $_rc
}}
if ! _tsm_health_check; then
    unset -f _tsm_health_check
    print -u2 "tsm: health check failed; tsm functionality disabled for this shell"
    return 0 2>/dev/null || exit 0
fi
unset -f _tsm_health_check

# Export session id. The literal hex below is generated fresh by `tsm shell-init`
# at print time, so each `eval "$(tsm shell-init zsh)"` yields a new id. If
# TSM_SESSION_ID is already set in the environment (parent shell exported it),
# the existing value is preserved.
: ${{TSM_SESSION_ID:={id}}}
export TSM_SESSION_ID

typeset -g _tsm_state_dir="${{XDG_STATE_HOME:-$HOME/.local/state}}/tsm"
typeset -g _tsm_fail_file="${{_tsm_state_dir}}/fail-count.$$"
typeset -g _tsm_error_log="${{_tsm_state_dir}}/errors.log"

_tsm_precmd() {{
    local _last_exit=$?
    local _last_cmd
    _last_cmd=$(fc -ln -1 2>/dev/null) || _last_cmd=""

    # Self-disable after {threshold} consecutive failures. The recorder writes
    # this counter; we read it synchronously before dispatching the next call.
    if [[ -r "$_tsm_fail_file" ]]; then
        local _fc
        _fc=$(<"$_tsm_fail_file")
        if [[ "$_fc" -ge {threshold} ]]; then
            add-zsh-hook -d precmd _tsm_precmd
            rm -f "$_tsm_fail_file"
            print -u2 "tsm: precmd hook disabled after {threshold} consecutive failures; see ${{_tsm_error_log}}"
            return 0
        fi
    fi

    # Dispatch recorder in the background and disown so the parent shell never
    # waits and the prompt is not delayed.
    {{ tsm record --exit-code "$_last_exit" --last-command "$_last_cmd" }} &!
}}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _tsm_precmd
# === tsm shell integration (end) ===
"#
    )
}

/// Resolve `$XDG_DATA_HOME` with the `$HOME/.local/share` fallback.
fn xdg_data_home() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("XDG_DATA_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local").join("share"))
}

/// Resolve `$XDG_STATE_HOME` with the `$HOME/.local/state` fallback.
fn xdg_state_home() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("XDG_STATE_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".local").join("state"))
}

/// Resolve the Zellij cache directory. Resolution order:
///
/// 1. `$ZELLIJ_CACHE_DIR`
/// 2. `$XDG_CACHE_HOME/zellij`
/// 3. `$HOME/.cache/zellij`
///
/// Returns `None` when none of the above can be determined.
fn zellij_cache_dir() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("ZELLIJ_CACHE_DIR") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    if let Ok(v) = std::env::var("XDG_CACHE_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v).join("zellij"));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| PathBuf::from(h).join(".cache").join("zellij"))
}

/// Compute the path to the active `session-layout.kdl` for the given Zellij
/// session, joining the cache dir with `<session>/session-layout.kdl`. Returns
/// `None` when no cache dir can be resolved.
fn zellij_session_layout_path(session: &str) -> Option<PathBuf> {
    zellij_cache_dir().map(|d| d.join(session).join("session-layout.kdl"))
}

/// Read Zellij session + pane env vars. Returns `None` when either is missing
/// or empty.
fn read_zellij_env() -> Option<TupleEnv> {
    let session = std::env::var("ZELLIJ_SESSION_NAME").ok()?;
    if session.is_empty() {
        return None;
    }
    let pane = std::env::var("ZELLIJ_PANE_ID").ok()?;
    if pane.is_empty() {
        return None;
    }
    Some(TupleEnv {
        zellij_session_name: session,
        zellij_pane_id: pane,
    })
}

/// Attempt to derive a deterministic [`SessionId`] from the current process
/// environment + on-disk Zellij `session-layout.kdl`.
///
/// Returns `None` when:
///
/// - either `ZELLIJ_SESSION_NAME` or `ZELLIJ_PANE_ID` is missing/empty
///   (outside-Zellij path),
/// - the layout file can't be located or read, or
/// - the layout can't produce a tuple (parse error, no tabs, missing pane,
///   etc — see [`TupleError`]).
///
/// This function is silent on failure: it MUST NOT print or log anything,
/// because it runs from `tsm shell-init zsh` where a stderr write would
/// disrupt the user's shell startup. The `tsm doctor` subcommand surfaces
/// the same information through a structured report instead.
fn try_derive_zellij_session_id() -> Option<SessionId> {
    let env = read_zellij_env()?;
    let layout_path = zellij_session_layout_path(&env.zellij_session_name)?;
    let layout_text = fs::read_to_string(&layout_path).ok()?;
    let tuple = derive_tuple(&env, &LayoutText(layout_text)).ok()?;
    Some(session_id_from_tuple(&tuple))
}

/// Pretty-format a derived [`Tuple`] for diagnostic output (`tsm doctor`).
///
/// Format: `(session=<name>, tab=<name>, ordinal=<N>)`.
fn format_tuple(tuple: &Tuple) -> String {
    format!(
        "(session={}, tab={}, ordinal={})",
        tuple.zellij_session_name,
        tuple.tab_name,
        tuple.pane_ordinal_within_tab,
    )
}

/// One observation row in the doctor report's "Layout file" section.
struct LayoutObservation {
    /// The resolved path, or `None` when the cache directory could not be
    /// determined at all (`$HOME` unset and no `$ZELLIJ_CACHE_DIR`).
    path: Option<PathBuf>,
    /// Whether the file exists on disk.
    exists: bool,
    /// File size in bytes; `None` when the file doesn't exist or stat failed.
    bytes: Option<u64>,
    /// File contents, if readable.
    contents: Option<String>,
}

/// Observe the layout file on disk for the doctor report. Pushes a
/// `layout file not found at ...` warning to `warnings` when the resolved
/// path does not exist.
fn observe_layout(session: &str, warnings: &mut Vec<String>) -> LayoutObservation {
    let mut obs = LayoutObservation {
        path: zellij_session_layout_path(session),
        exists: false,
        bytes: None,
        contents: None,
    };
    if let Some(p) = obs.path.as_deref() {
        if let Ok(meta) = fs::metadata(p) {
            obs.exists = true;
            obs.bytes = Some(meta.len());
            obs.contents = fs::read_to_string(p).ok();
        } else {
            obs.exists = false;
            warnings.push(format!("layout file not found at {}", p.display()));
        }
    }
    obs
}

/// Tuple-derivation step of the doctor report. Returns a (status, detail,
/// tuple) triple and may push extra warnings.
fn derive_for_doctor(
    session_env: Option<&str>,
    pane_env: Option<&str>,
    layout: &LayoutObservation,
    warnings: &mut Vec<String>,
) -> (String, String, Option<Tuple>) {
    let (Some(session), Some(pane)) = (session_env, pane_env) else {
        return (
            "skipped".to_string(),
            "outside Zellij — random id path".to_string(),
            None,
        );
    };
    let Some(text) = layout.contents.as_deref() else {
        return (
            "skipped".to_string(),
            "layout file unreadable or missing".to_string(),
            None,
        );
    };
    let env_inputs = TupleEnv {
        zellij_session_name: session.to_string(),
        zellij_pane_id: pane.to_string(),
    };
    match derive_tuple(&env_inputs, &LayoutText(text.to_string())) {
        Ok(t) => {
            let pretty = format_tuple(&t);
            ("ok".to_string(), pretty, Some(t))
        }
        Err(e) => {
            let msg = e.to_string();
            if matches!(&e, TupleError::AmbiguousPaneId { .. }) {
                warnings.push("pane id is ambiguous in layout".to_string());
            }
            warnings.push(format!("tuple derivation failed: {msg}"));
            ("error".to_string(), msg, None)
        }
    }
}

/// Aggregated inputs to [`render_doctor_report`].
struct DoctorReport<'a> {
    session_display: &'a str,
    pane_display: &'a str,
    layout: &'a LayoutObservation,
    status: &'a str,
    detail: &'a str,
    id_source: &'a str,
    id_value: &'a str,
    warnings: &'a [String],
}

/// Render the structured doctor report to a `String`. The output is plain
/// text designed to be readable by humans and parse-friendly for tests.
fn render_doctor_report(report: &DoctorReport<'_>) -> String {
    let DoctorReport {
        session_display,
        pane_display,
        layout,
        status,
        detail,
        id_source,
        id_value,
        warnings,
    } = *report;
    let path_display = layout
        .path
        .as_deref()
        .map_or_else(|| "(unresolved)".to_string(), |p| p.display().to_string());
    let exists_display = if layout.path.is_none() {
        "-"
    } else if layout.exists {
        "yes"
    } else {
        "no"
    };
    let bytes_display = layout
        .bytes
        .map_or_else(|| "-".to_string(), |b| b.to_string());

    let mut out = String::new();
    let _ = writeln!(out, "tsm doctor");
    let _ = writeln!(out);
    let _ = writeln!(out, "Zellij environment:");
    let _ = writeln!(out, "  ZELLIJ_SESSION_NAME = {session_display}");
    let _ = writeln!(out, "  ZELLIJ_PANE_ID      = {pane_display}");
    let _ = writeln!(out);
    let _ = writeln!(out, "Layout file:");
    let _ = writeln!(out, "  path     = {path_display}");
    let _ = writeln!(out, "  exists   = {exists_display}");
    let _ = writeln!(out, "  bytes    = {bytes_display}");
    let _ = writeln!(out);
    let _ = writeln!(out, "Tuple derivation:");
    let _ = writeln!(out, "  status   = {status}");
    let _ = writeln!(out, "  detail   = {detail}");
    let _ = writeln!(out);
    let _ = writeln!(out, "Session id:");
    let _ = writeln!(out, "  source   = {id_source}");
    let _ = writeln!(out, "  value    = {id_value}");
    let _ = writeln!(out);
    let _ = writeln!(out, "Warnings:");
    if warnings.is_empty() {
        let _ = writeln!(out, "  none");
    } else {
        for w in warnings {
            let _ = writeln!(out, "  {w}");
        }
    }
    out
}

/// Compute the `(source, value)` pair for the "Session id" section of the
/// report. `source` is one of `deterministic`, `random`, or `none`.
fn session_id_for_doctor(
    derived: Option<&Tuple>,
    in_zellij: bool,
) -> (String, String) {
    if let Some(t) = derived {
        (
            "deterministic".to_string(),
            session_id_from_tuple(t).as_hex().to_string(),
        )
    } else if in_zellij {
        // We're inside Zellij but failed somewhere. The runtime fallback in
        // main() would have used a random id, but reporting the actual hex
        // would lie about determinism — emit `none` instead.
        ("none".to_string(), "-".to_string())
    } else {
        ("random".to_string(), "-".to_string())
    }
}

/// Render the `tsm doctor` report to stdout. Always exits 0 on the caller's
/// behalf — every failure shows up in the report rather than as an error.
fn handle_doctor() {
    let mut warnings: Vec<String> = Vec::new();
    let session_env = std::env::var("ZELLIJ_SESSION_NAME").ok();
    let pane_env = std::env::var("ZELLIJ_PANE_ID").ok();
    let session_set = session_env.as_deref().is_some_and(|v| !v.is_empty());
    let pane_set = pane_env.as_deref().is_some_and(|v| !v.is_empty());

    let session_display = if session_set {
        session_env.clone().unwrap_or_default()
    } else {
        "(unset)".to_string()
    };
    let pane_display = if pane_set {
        pane_env.clone().unwrap_or_default()
    } else {
        "(unset)".to_string()
    };

    if session_set != pane_set {
        warnings.push(
            "ZELLIJ_SESSION_NAME is set but ZELLIJ_PANE_ID is not (or vice versa)"
                .to_string(),
        );
    }

    let layout = if session_set {
        observe_layout(session_env.as_deref().unwrap_or(""), &mut warnings)
    } else {
        LayoutObservation {
            path: None,
            exists: false,
            bytes: None,
            contents: None,
        }
    };

    let session_for_derive = if session_set { session_env.as_deref() } else { None };
    let pane_for_derive = if pane_set { pane_env.as_deref() } else { None };
    let (status, detail, derived_tuple) = derive_for_doctor(
        session_for_derive,
        pane_for_derive,
        &layout,
        &mut warnings,
    );

    let (id_source, id_value) =
        session_id_for_doctor(derived_tuple.as_ref(), session_set && pane_set);

    let out = render_doctor_report(&DoctorReport {
        session_display: &session_display,
        pane_display: &pane_display,
        layout: &layout,
        status: &status,
        detail: &detail,
        id_source: &id_source,
        id_value: &id_value,
        warnings: &warnings,
    });
    print!("{out}");
}

/// Compute the session log path for `<session-id>`. Creates parent dirs with
/// mode 0700 on the `tsm/` directory.
fn session_log_path(session_id: &SessionId) -> Option<PathBuf> {
    let data = xdg_data_home()?;
    let tsm_dir = data.join("tsm");
    let sessions_dir = tsm_dir.join("sessions");
    fs::create_dir_all(&sessions_dir).ok()?;
    // Best-effort: tighten permissions on the tsm/ root to 0700. We don't fail
    // if this errors — the session log is the load-bearing artifact.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(&tsm_dir) {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            let _ = fs::set_permissions(&tsm_dir, perms);
        }
    }
    Some(sessions_dir.join(format!("{}.jsonl", session_id.as_hex())))
}

/// Compute the error log path. Creates parent dir if missing.
fn error_log_path_in(state_dir: &Path) -> PathBuf {
    let tsm_dir = state_dir.join("tsm");
    let _ = fs::create_dir_all(&tsm_dir);
    tsm_dir.join("errors.log")
}

/// Compute the fail-counter file path for our parent process.
fn fail_count_path_in(state_dir: &Path) -> PathBuf {
    let tsm_dir = state_dir.join("tsm");
    let _ = fs::create_dir_all(&tsm_dir);
    // On Unix `parent_id` is always available; if we're somehow init (PPID 0)
    // we fall back to our own pid so the path is still unique.
    let ppid = parent_pid();
    tsm_dir.join(format!("fail-count.{ppid}"))
}

/// Return the parent process id. Falls back to our own pid if PPID is 0.
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        let p = std::os::unix::process::parent_id();
        if p == 0 { std::process::id() } else { p }
    }
    #[cfg(not(unix))]
    {
        std::process::id()
    }
}

/// Truncate the error log to keep only the last `ERROR_LOG_KEEP_BYTES` bytes
/// if it has grown past `ERROR_LOG_MAX_BYTES`. Best-effort; failures swallowed.
fn rotate_error_log_if_needed(path: &Path) {
    let Ok(meta) = fs::metadata(path) else { return };
    if meta.len() <= ERROR_LOG_MAX_BYTES {
        return;
    }
    // Read the tail of the file and rename a `.tmp` sibling over the original.
    let Ok(mut f) = File::open(path) else { return };
    let len = meta.len();
    let keep = u64::try_from(ERROR_LOG_KEEP_BYTES).unwrap_or(0);
    let start = len.saturating_sub(keep);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return;
    }
    let mut buf = Vec::with_capacity(ERROR_LOG_KEEP_BYTES);
    if f.read_to_end(&mut buf).is_err() {
        return;
    }
    // Drop everything up to the first newline so we don't have a partial line.
    if let Some(nl) = buf.iter().position(|b| *b == b'\n') {
        buf.drain(..=nl);
    }
    let tmp = path.with_extension(format!("log.tmp.{}", std::process::id()));
    let Ok(mut out) = File::create(&tmp) else { return };
    if out.write_all(&buf).is_err() {
        let _ = fs::remove_file(&tmp);
        return;
    }
    let _ = fs::rename(&tmp, path);
}

/// Append `msg` to the error log at `path`, prefixed with an RFC3339 timestamp.
/// Best-effort: any failure is silently dropped (we cannot recurse on logging).
fn log_error_to(path: &Path, msg: &str) {
    rotate_error_log_if_needed(path);
    let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let ts = Utc::now().to_rfc3339();
    let _ = writeln!(f, "{ts} {msg}");
}

/// Atomically write `value` to the fail-counter file at `path`. Uses a tmp
/// sibling + rename so a partial write is never observed.
fn write_fail_counter(path: &Path, value: u32) {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Ok(mut f) = File::create(&tmp) {
        if writeln!(f, "{value}").is_ok() {
            let _ = fs::rename(&tmp, path);
            return;
        }
    }
    let _ = fs::remove_file(&tmp);
}

/// Read the current fail counter (0 if absent or unparseable).
fn read_fail_counter(path: &Path) -> u32 {
    let Ok(s) = fs::read_to_string(path) else { return 0 };
    s.trim().parse::<u32>().unwrap_or(0)
}

/// Bump the fail counter by 1, atomically.
fn bump_fail_counter(state_dir: &Path) {
    let path = fail_count_path_in(state_dir);
    let v = read_fail_counter(&path).saturating_add(1);
    write_fail_counter(&path, v);
}

/// Reset the fail counter to 0.
fn reset_fail_counter(state_dir: &Path) {
    let path = fail_count_path_in(state_dir);
    write_fail_counter(&path, 0);
}

/// Run `cmd` with a watchdog timeout. On timeout, the child is killed and
/// reaped; we return `Err(())`. We intentionally do not expose the underlying
/// error type — the recorder swallows all subprocess errors anyway.
fn run_with_timeout(mut cmd: ProcCommand, timeout: Duration) -> Result<(), ()> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ())?;
    if child.wait_timeout(timeout).map_err(|_| ())?.is_some() {
        Ok(())
    } else {
        // Timeout: kill and reap. Both errors are swallowed.
        let _ = child.kill();
        let _ = child.wait();
        Err(())
    }
}

/// Parse a free-form `--probe-subprocess "/bin/sleep 5"` string into a
/// `Command`. Whitespace-split, no shell. The first token is the program;
/// subsequent tokens are args.
fn parse_probe_command(spec: &str) -> Option<ProcCommand> {
    let mut tokens = spec.split_whitespace();
    let prog = tokens.next()?;
    let mut cmd = ProcCommand::new(prog);
    for arg in tokens {
        cmd.arg(arg);
    }
    Some(cmd)
}

/// Build the Header used for line 1 of a fresh session log.
fn build_header() -> Header {
    let hostname = hostname::get().map_or_else(
        |_| "unknown".to_string(),
        |h| h.to_string_lossy().into_owned(),
    );
    let terminal_program =
        std::env::var("TERM_PROGRAM").unwrap_or_else(|_| "unknown".to_string());
    Header {
        kind: HeaderKind::Header,
        schema_version: 1,
        tsm_version: version_string!().to_string(),
        hostname,
        terminal_program,
        tuple: TupleStub {
            zellij_session: None,
            tab: None,
            pane_ordinal_str: None,
        },
        created_at: Utc::now().to_rfc3339(),
    }
}

/// Build a `PrecmdRecord` from the current process environment plus the
/// caller-supplied exit code and last-command text.
fn build_record(exit_code: i32, last_command: String) -> PrecmdRecord {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let (env, redacted_keys) = redact_env(env);
    PrecmdRecord {
        kind: PrecmdKind::Precmd,
        at: Utc::now().to_rfc3339(),
        cwd,
        exit_code,
        last_command,
        env,
        redacted_keys,
    }
}

/// Append a record. If the file is empty / missing, write a header first and
/// then retry. Handles a concurrent-writer race on the header by treating
/// `DuplicateHeader` as success.
fn append_record_with_header(path: &Path, record: &PrecmdRecord) -> Result<(), JsonlError> {
    match append_record(path, record) {
        Ok(()) => Ok(()),
        Err(JsonlError::MissingHeader) => {
            let header = build_header();
            match append_header(path, &header) {
                Ok(()) | Err(JsonlError::DuplicateHeader) => append_record(path, record),
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    }
}

/// The core of the recorder. Returns `Ok(())` on success, `Err(String)` on
/// any internal failure (the caller logs the message and bumps the counter).
fn do_record(
    state_dir: &Path,
    exit_code: i32,
    last_command: String,
) -> Result<(), String> {
    let raw_id = std::env::var("TSM_SESSION_ID")
        .map_err(|_| "record: TSM_SESSION_ID is not set".to_string())?;
    let session_id = SessionId::from_hex(&raw_id)
        .map_err(|e| format!("record: invalid TSM_SESSION_ID: {e}"))?;

    let _ = state_dir; // path resolution for the session log uses XDG_DATA_HOME.
    let log_path = session_log_path(&session_id)
        .ok_or_else(|| "record: could not resolve session log path".to_string())?;

    // First call: file is empty/missing → write header.
    let first_call = match fs::metadata(&log_path) {
        Ok(m) => m.len() == 0,
        Err(_) => true,
    };

    if first_call {
        let header = build_header();
        match append_header(&log_path, &header) {
            Ok(()) | Err(JsonlError::DuplicateHeader) => {}
            Err(e) => return Err(format!("record: append_header failed: {e}")),
        }
        return Ok(());
    }

    let record = build_record(exit_code, last_command);
    append_record_with_header(&log_path, &record)
        .map_err(|e| format!("record: append_record failed: {e}"))?;
    Ok(())
}

fn handle_record(
    exit_code: i32,
    last_command: String,
    probe_subprocess: Option<String>,
) {
    let state_dir = xdg_state_home();
    let err_log = state_dir.as_deref().map(error_log_path_in);

    // Probe path: synthetic subprocess for the watchdog acceptance test. We do
    // NOT touch the session log on this path.
    if let Some(spec) = probe_subprocess {
        if let Some(cmd) = parse_probe_command(&spec) {
            if run_with_timeout(cmd, SUBPROCESS_TIMEOUT).is_err() {
                if let Some(p) = err_log.as_deref() {
                    log_error_to(
                        p,
                        &format!("record: probe subprocess timed out after 2s: {spec}"),
                    );
                }
            }
        } else if let Some(p) = err_log.as_deref() {
            log_error_to(p, &format!("record: probe-subprocess empty: {spec}"));
        }
        return;
    }

    match do_record(state_dir.as_deref().unwrap_or(Path::new(".")), exit_code, last_command) {
        Ok(()) => {
            if let Some(dir) = state_dir.as_deref() {
                reset_fail_counter(dir);
            }
        }
        Err(msg) => {
            if let Some(p) = err_log.as_deref() {
                log_error_to(p, &msg);
            }
            if let Some(dir) = state_dir.as_deref() {
                bump_fail_counter(dir);
            }
        }
    }
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::ShellInit { shell } => {
            if shell != "zsh" {
                eprintln!(
                    "tsm: shell-init: only \"zsh\" is supported in v1, got: {shell}"
                );
                std::process::exit(EXIT_UNSUPPORTED_SHELL);
            }
            // Inside Zellij with a readable session-layout.kdl, derive a
            // deterministic SessionId from the (session, tab, pane-ordinal)
            // tuple so resurrecting the pane yields the same id. Anywhere
            // else we fall back to a random id; this includes derivation
            // errors, which are surfaced through `tsm doctor` rather than
            // stderr-from-shell-init.
            let session_id = match try_derive_zellij_session_id() {
                Some(id) => id,
                None => SessionId::random(),
            };
            print!("{}", generate_zsh_snippet(&session_id));
        }
        Commands::Doctor => {
            handle_doctor();
        }
        Commands::Record {
            exit_code,
            last_command,
            probe_subprocess,
        } => {
            // Safety contract: never panic, never write stderr from the hot
            // path. Internal failures go to the error log; the fail counter
            // ticks up; we always exit 0.
            handle_record(exit_code, last_command, probe_subprocess);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fixed session ID for deterministic snippet inspection.
    fn fixed_session_id() -> SessionId {
        SessionId::from_hex("0123456789abcdef0123456789abcdef")
            .expect("fixed hex is valid")
    }

    #[test]
    fn zsh_snippet_starts_with_tsm_disable_guard() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("TSM_DISABLE"),
            "snippet should guard on $TSM_DISABLE near the top, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_inlines_session_id() {
        let id = fixed_session_id();
        let s = generate_zsh_snippet(&id);
        let hex = id.as_hex();
        let occurrences = s.matches(hex).count();
        assert_eq!(
            occurrences, 1,
            "expected session id {hex} to appear exactly once in snippet, got {occurrences}:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_uses_random_id_when_called_via_main() {
        let a = generate_zsh_snippet(&SessionId::random());
        let b = generate_zsh_snippet(&SessionId::random());
        assert_ne!(
            a, b,
            "two snippets generated with random ids should differ"
        );
    }

    #[test]
    fn zsh_snippet_uses_disowned_call() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("&!"),
            "snippet should background-disown the tsm record call with &!, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_passes_exit_code_and_last_command() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("--exit-code"),
            "snippet should pass --exit-code to tsm record, got:\n{s}"
        );
        assert!(
            s.contains("--last-command"),
            "snippet should pass --last-command to tsm record, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_registers_precmd_hook() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("add-zsh-hook precmd"),
            "snippet should register the precmd hook via add-zsh-hook, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_health_check_present() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("tsm --version"),
            "snippet should perform a health check by calling tsm --version, got:\n{s}"
        );
        assert!(
            s.contains("sleep 1") || s.contains("sleep 1\n") || s.contains("sleep 1 "),
            "snippet should bound the health check with a ~1 second timeout, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_self_disables_after_3_failures() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("add-zsh-hook -d precmd"),
            "snippet should unregister the precmd hook on repeated failure, got:\n{s}"
        );
        assert!(
            s.contains('3'),
            "snippet should reference the failure threshold of 3, got:\n{s}"
        );
    }

    #[test]
    fn zsh_snippet_does_not_emit_osc_sequences() {
        let s = generate_zsh_snippet(&fixed_session_id());
        for needle in ["\\033]", "\\x1b]", "\\e]"] {
            assert!(
                !s.contains(needle),
                "snippet should not emit OSC sequences (found {needle}), got:\n{s}"
            );
        }
    }

    #[test]
    fn zsh_snippet_uses_xdg_state_home_fallback() {
        let s = generate_zsh_snippet(&fixed_session_id());
        assert!(
            s.contains("${XDG_STATE_HOME:-$HOME/.local/state}"),
            "snippet should compute state dir via XDG_STATE_HOME fallback, got:\n{s}"
        );
    }

    // ----- redaction unit tests (red commit will see these fail against stubs) -----

    #[test]
    fn is_redacted_matches_suffix_patterns() {
        assert!(is_redacted("AWS_SESSION_TOKEN"));
        assert!(is_redacted("MY_API_KEY"));
        assert!(is_redacted("DB_PASSWORD"));
        assert!(is_redacted("OPENAI_API_KEY"));
    }

    #[test]
    fn is_redacted_case_insensitive() {
        assert!(is_redacted("aws_session_token"));
        assert!(is_redacted("github_token"));
        assert!(is_redacted("my_secret"));
    }

    #[test]
    fn is_redacted_does_not_match_plain_names() {
        assert!(!is_redacted("HOME"));
        assert!(!is_redacted("PATH"));
        assert!(!is_redacted("USER"));
        assert!(!is_redacted("SHELL"));
    }

    #[test]
    fn is_redacted_prefix_aws() {
        assert!(is_redacted("AWS_REGION"));
        assert!(is_redacted("AWS_PROFILE"));
        assert!(is_redacted("AWS_DEFAULT_REGION"));
    }

    #[test]
    fn is_redacted_prefix_op() {
        assert!(is_redacted("OP_SESSION"));
        assert!(is_redacted("OP_DEVICE"));
    }

    #[test]
    fn is_redacted_anthropic_exact() {
        assert!(is_redacted("ANTHROPIC_API_KEY"));
        assert!(is_redacted("GH_TOKEN"));
        assert!(is_redacted("GITHUB_TOKEN"));
    }

    #[test]
    fn is_redacted_claude_prefix() {
        assert!(is_redacted("CLAUDE_CODE_THING"));
        assert!(is_redacted("CLAUDE_API_KEY"));
    }

    #[test]
    fn redact_env_filters_keys_and_keeps_redacted_keys_list() {
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/x".to_string());
        env.insert("AWS_SESSION_TOKEN".to_string(), "secret1".to_string());
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        env.insert("MY_API_KEY".to_string(), "secret2".to_string());

        let (filtered, redacted_keys) = redact_env(env);

        assert!(filtered.contains_key("HOME"));
        assert!(filtered.contains_key("PATH"));
        assert!(!filtered.contains_key("AWS_SESSION_TOKEN"));
        assert!(!filtered.contains_key("MY_API_KEY"));
        assert!(redacted_keys.contains(&"AWS_SESSION_TOKEN".to_string()));
        assert!(redacted_keys.contains(&"MY_API_KEY".to_string()));
        assert_eq!(redacted_keys.len(), 2);
    }

    #[test]
    fn redact_env_redacted_keys_is_sorted() {
        let mut env = BTreeMap::new();
        env.insert("Z_TOKEN".to_string(), "v".to_string());
        env.insert("A_SECRET".to_string(), "v".to_string());
        env.insert("M_PASSWORD".to_string(), "v".to_string());
        env.insert("HOME".to_string(), "v".to_string());

        let (_filtered, redacted_keys) = redact_env(env);
        let mut sorted = redacted_keys.clone();
        sorted.sort();
        assert_eq!(redacted_keys, sorted, "redacted_keys must be sorted");
        assert_eq!(redacted_keys.len(), 3);
    }
}
