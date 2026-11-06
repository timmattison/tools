//! The one entrance to the `op` CLI. Each run of `op` goes through
//! [`OpRun::attempt`], which writes the audit trail around it.

use crate::audit::{self, AuditTrail, OpEnd, OpStart, Outcome};
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
    #[expect(dead_code, reason = "the read-binary wiring is a later slice")]
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
}

/// The environment variables op-cache reads, read once.
struct Environment {
    xdg_state_home: Option<OsString>,
    home: Option<OsString>,
    zellij_session_name: Option<OsString>,
    zellij_pane_id: Option<OsString>,
    term_program: Option<OsString>,
}

impl Environment {
    fn snapshot() -> Self {
        Self {
            xdg_state_home: std::env::var_os("XDG_STATE_HOME"),
            home: std::env::var_os("HOME"),
            zellij_session_name: std::env::var_os("ZELLIJ_SESSION_NAME"),
            zellij_pane_id: std::env::var_os("ZELLIJ_PANE_ID"),
            term_program: std::env::var_os("TERM_PROGRAM"),
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
    environment: Environment,
    trail: Option<AuditTrail>,
    trail_given_up: bool,
}

impl<'a> OpRun<'a> {
    pub(crate) fn new(operation: Operation<'a>, op_path: &'a OpPath, cache_path: &'a Path) -> Self {
        Self {
            operation,
            op_path,
            cache_path,
            environment: Environment::snapshot(),
            trail: None,
            trail_given_up: false,
        }
    }

    /// One attempt: writes `op_start`, runs `op`, writes `op_end`. Returns what
    /// `Command::output()` returned, unchanged.
    pub(crate) fn attempt(&mut self, attempt: u32) -> std::io::Result<Output> {
        let request_id = new_request_id();
        let start = OpStart {
            time: now(),
            event: "op_start",
            request_id: request_id.clone(),
            attempt,
            operation: self.operation.audit_name(),
            reference: self.op_path.to_string(),
            cwd: std::env::current_dir()
                .ok()
                .map(|dir| dir.to_string_lossy().into_owned()),
            cache_path: self.cache_path.to_string_lossy().into_owned(),
            zellij_session: lossy(self.environment.zellij_session_name.as_ref()),
            zellij_pane: lossy(self.environment.zellij_pane_id.as_ref()),
            term_program: lossy(self.environment.term_program.as_ref()),
            claude: None,
            ancestors: vec![],
        };
        self.record(&start);

        let started = Instant::now();
        let result = Command::new("op")
            .args(self.operation.args(self.op_path.as_ref()))
            .output();
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);

        let (exit_code, outcome) = match &result {
            Ok(output) if output.status.success() => (output.status.code(), Outcome::Ok),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                (
                    output.status.code(),
                    Outcome::Failed {
                        op_stderr: audit::truncate_at_char_boundary(
                            stderr.trim_end(),
                            OP_STDERR_MAX_BYTES,
                        )
                        .to_owned(),
                    },
                )
            }
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
