//! The one entrance to the `op` CLI. Each run of `op` goes through
//! [`OpRun::attempt`], which writes the audit trail around it.

use crate::audit::{self, AuditTrail, OpEnd, OpStart, Outcome};
use crate::caller::{self, Caller};
use crate::notify;
use crate::OpPath;
use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The most bytes of `op` standard error that one `op_end` record keeps.
const OP_STDERR_MAX_BYTES: usize = 512;

/// What to ask `op` for.
pub(crate) enum Operation<'a> {
    /// `op read <reference>`
    Read,
    /// `op read --out-file <output_path> <reference>`
    ReadBinary { output_path: &'a Path },
}

impl Operation<'_> {
    /// The name written to the audit trail.
    fn audit_name(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ReadBinary { .. } => "read-binary",
        }
    }

    /// The arguments of `op` for `reference`.
    fn args(&self, reference: &str) -> Vec<String> {
        match self {
            Self::Read => vec!["read".to_owned(), reference.to_owned()],
            Self::ReadBinary { output_path } => vec![
                "read".to_owned(),
                "--out-file".to_owned(),
                output_path.to_string_lossy().into_owned(),
                reference.to_owned(),
            ],
        }
    }

    /// True when `output` carries the secret: `op` exited 0 and the value is
    /// not empty. For `read`, the value is the standard output without its
    /// surrounding whitespace. For `read-binary`, it is the file at
    /// `output_path`.
    fn gave_value(&self, output: &Output) -> bool {
        output.status.success()
            && match self {
                Self::Read => !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
                Self::ReadBinary { output_path } => output_path
                    .metadata()
                    .is_ok_and(|metadata| metadata.len() > 0),
            }
    }
}

/// The standard error of `op` without trailing whitespace, cut to
/// `OP_STDERR_MAX_BYTES` on a char boundary.
fn op_stderr_excerpt(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    audit::truncate_at_char_boundary(stderr.trim_end(), OP_STDERR_MAX_BYTES).to_owned()
}

/// The environment variables op-cache reads, read once.
struct Environment {
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
    claude_config_dir: Option<OsString>,
    zellij_session_name: Option<OsString>,
    zellij_pane_id: Option<OsString>,
    term_program: Option<OsString>,
    op_cache_notify: Option<OsString>,
}

impl Environment {
    fn snapshot() -> Self {
        Self {
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            home: std::env::var_os("HOME"),
            claude_config_dir: std::env::var_os("CLAUDE_CONFIG_DIR"),
            zellij_session_name: std::env::var_os("ZELLIJ_SESSION_NAME"),
            zellij_pane_id: std::env::var_os("ZELLIJ_PANE_ID"),
            term_program: std::env::var_os("TERM_PROGRAM"),
            op_cache_notify: std::env::var_os("OP_CACHE_NOTIFY"),
        }
    }
}

fn lossy(value: Option<&OsString>) -> Option<String> {
    value.map(|text| text.to_string_lossy().into_owned())
}

/// One request for a secret, which may take several attempts.
pub(crate) struct OpRun<'a> {
    operation: Operation<'a>,
    op_path: &'a OpPath,
    cache_path: &'a Path,
    max_attempts: u32,
    environment: Environment,
    caller: Option<Caller>,
    trail: Option<AuditTrail>,
    trail_given_up: bool,
    notification_warned: bool,
}

impl<'a> OpRun<'a> {
    /// `max_attempts` is the number the caller will try at most. The
    /// notification title shows it on a retry.
    pub(crate) fn new(
        operation: Operation<'a>,
        op_path: &'a OpPath,
        cache_path: &'a Path,
        max_attempts: u32,
    ) -> Self {
        Self {
            operation,
            op_path,
            cache_path,
            max_attempts,
            environment: Environment::snapshot(),
            caller: None,
            trail: None,
            trail_given_up: false,
            notification_warned: false,
        }
    }

    /// True when `output`, from [`OpRun::attempt`], carries the secret. The
    /// audit trail uses the same rule, so an `ok` record and a returned value
    /// always agree.
    pub(crate) fn gave_value(&self, output: &Output) -> bool {
        self.operation.gave_value(output)
    }

    /// One attempt: writes `op_start`, starts the notification, runs `op`,
    /// reaps the notification, and writes `op_end`. Returns what
    /// `Command::output()` returned, unchanged.
    pub(crate) fn attempt(&mut self, attempt: u32) -> std::io::Result<Output> {
        let request_id = new_request_id();
        let environment = &self.environment;
        let caller = self.caller.get_or_insert_with(|| {
            caller::gather(
                environment.claude_config_dir.as_deref(),
                environment.home.as_deref(),
            )
        });
        let start = OpStart {
            time: now(),
            event: "op_start",
            request_id: request_id.clone(),
            attempt,
            operation: self.operation.audit_name(),
            reference: self.op_path.to_string(),
            cwd: caller.cwd.clone(),
            cache_path: self.cache_path.to_string_lossy().into_owned(),
            zellij_session: lossy(self.environment.zellij_session_name.as_ref()),
            zellij_pane: lossy(self.environment.zellij_pane_id.as_ref()),
            term_program: lossy(self.environment.term_program.as_ref()),
            claude: caller.claude.clone(),
            ancestors: caller.ancestors.clone(),
        };
        self.record(&start);

        let notification = self.start_notification(attempt);

        let started = Instant::now();
        let result = Command::new("op")
            .args(self.operation.args(self.op_path.as_ref()))
            .output();
        // Read the clock before the reap, so that `duration_ms` measures `op`
        // alone and not the time to stop osascript.
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(child) = notification {
            if let Err(what) = notify::finish(child) {
                self.warn_notification(&what);
            }
        }

        let (exit_code, outcome) = match &result {
            Ok(output) if self.operation.gave_value(output) => (output.status.code(), Outcome::Ok),
            Ok(output) if output.status.success() => (
                output.status.code(),
                Outcome::Empty {
                    op_stderr: op_stderr_excerpt(output),
                },
            ),
            Ok(output) => (
                output.status.code(),
                Outcome::Failed {
                    op_stderr: op_stderr_excerpt(output),
                },
            ),
            Err(error) => (
                None,
                Outcome::SpawnError {
                    error: error.to_string(),
                },
            ),
        };
        let end = OpEnd {
            time: now(),
            event: "op_end",
            request_id,
            attempt,
            exit_code,
            duration_ms,
            outcome,
        };
        self.record(&end);

        result
    }

    /// Starts the notification of one attempt, unless `OP_CACHE_NOTIFY=0`.
    /// A failure prints at most one warning for this request and never stops
    /// the read.
    fn start_notification(&mut self, attempt: u32) -> Option<std::process::Child> {
        if !notify::notifications_enabled(self.environment.op_cache_notify.as_deref()) {
            return None;
        }
        let claude_name = self
            .caller
            .as_ref()
            .and_then(|caller| caller.claude.as_ref())
            .and_then(|claude| claude.name.as_deref());
        let zellij_session = self
            .environment
            .zellij_session_name
            .as_ref()
            .map(|name| name.to_string_lossy());
        let repo_dir_name = self
            .cache_path
            .parent()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy());
        let notification = notify::compose(
            attempt,
            self.max_attempts,
            self.op_path.as_ref(),
            claude_name,
            zellij_session.as_deref(),
            repo_dir_name.as_deref(),
        );
        match notify::start(&notification) {
            Ok(child) => child,
            Err(error) => {
                self.warn_notification(&format!("cannot start osascript: {error}"));
                None
            }
        }
    }

    /// Prints the first notification failure of this request, and no other.
    /// Each cache miss makes a new [`OpRun`], thus a process that misses three
    /// secrets can print three warnings.
    fn warn_notification(&mut self, what: &str) {
        if !self.notification_warned {
            self.notification_warned = true;
            eprintln!("warning: op-cache: notification: {what}");
        }
    }

    /// Appends a record. A failure prints a warning and never stops the read.
    fn record<T: serde::Serialize>(&mut self, record: &T) {
        if self.trail_given_up {
            return;
        }
        if let Err(what) = self.try_record(record) {
            self.trail_given_up = true;
            eprintln!("warning: op-cache: {what}");
        }
    }

    fn try_record<T: serde::Serialize>(&mut self, record: &T) -> Result<(), String> {
        if self.trail.is_none() {
            let path = audit::audit_path(
                self.environment.xdg_state_home.as_deref(),
                self.environment.home.as_deref(),
            )
            .ok_or_else(|| {
                "no audit trail: neither XDG_STATE_HOME nor HOME is usable".to_owned()
            })?;
            let trail = AuditTrail::open(&path).map_err(|error| {
                format!("cannot open the audit trail {}: {error}", path.display())
            })?;
            self.trail = Some(trail);
        }
        match self.trail.as_mut() {
            Some(trail) => trail
                .append(record)
                .map_err(|error| format!("cannot write the audit trail: {error}")),
            None => Ok(()),
        }
    }
}

/// `2026-11-06T15:38:24.333Z`
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A new id for one attempt: process id and nanoseconds since the epoch.
fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    format!("{}-{nanos}", std::process::id())
}
