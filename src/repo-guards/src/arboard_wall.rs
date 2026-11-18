//! Guard: only `sysclip` may depend on `arboard`.
//!
//! The crate `sysclip` is the one entrance to the clipboard of the machine. It
//! applies the order rule and uses the mosh-rs shims. A member that depends on
//! `arboard` itself can bypass both. Thus `sysclip` is the only workspace
//! member that may name `arboard` in a dependency table.
//!
//! [`audit`] runs `cargo metadata --no-deps --offline` and reads its JSON. It
//! does not read the manifests as text. The metadata answers three questions
//! that a text match gets wrong:
//!
//! - **Package name or local name.** `clip = { package = "arboard" }` depends
//!   on `arboard` under another name. `arboard = { package = "copypasta" }`
//!   does not. Cargo reports the package as `name` and the local name as
//!   `rename`, so the guard reads `name`.
//! - **Target tables.** An entry in `[target.'cfg(unix)'.dependencies]` is a
//!   dependency. Cargo reports it with its `target`.
//! - **Workspace inheritance.** `arboard.workspace = true` has no version in
//!   the member manifest. Cargo resolves it before it reports.
//!
//! Every `kind` is an offense: normal, `dev` and `build`. An unknown kind is
//! recorded as the raw string and is also an offense.
//!
//! # Fail closed
//!
//! A guard that finds no owner proves nothing. If no workspace member is named
//! [`OWNER`], the audit returns [`ArboardWallError::OwnerMissing`]. The same
//! holds when cargo cannot start, exits with an error, writes output that is
//! not JSON, or writes JSON without the fields that the guard reads. None of
//! these cases gives a clean report.
//!
//! # What this guard does not prove
//!
//! - A dependency of a dependency. The guard reads direct dependencies only
//!   (`--no-deps`).
//! - A crate that reaches the clipboard another way, for example when it runs
//!   `pbcopy` itself.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::Value;
use thiserror::Error;

/// The package that no member but the owner may depend on.
pub const GUARDED_PACKAGE: &str = "arboard";

/// The one member that may depend on [`GUARDED_PACKAGE`].
pub const OWNER: &str = "sysclip";

/// Environment variable that names the cargo of the running toolchain.
const CARGO_ENV: &str = "CARGO";

/// Program that starts when [`CARGO_ENV`] is not set.
const CARGO_FALLBACK: &str = "cargo";

/// Arguments of the metadata query. `--no-deps` and `--offline` need no network.
const METADATA_ARGS: [&str; 5] = [
    "metadata",
    "--no-deps",
    "--offline",
    "--format-version",
    "1",
];

/// Variables that change what cargo reads or where it writes.
const SCRUBBED_VARS: [&str; 5] = [
    "CARGO_ENCODED_RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_TARGET_DIR",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
];

/// Every variable with this prefix is removed too. It is the environment form
/// of the `[build]` table of the cargo configuration.
const SCRUBBED_PREFIX: &str = "CARGO_BUILD_";

/// Everything that can stop the audit from reaching a verdict.
///
/// Each variant is a refusal. None of them gives a clean report.
#[derive(Debug, Error)]
pub enum ArboardWallError {
    /// Cargo could not start.
    #[error("cannot start `{program}` in {}: {source}", dir.display())]
    Spawn {
        /// The program that failed to start.
        program: String,
        /// The directory of the run.
        dir: PathBuf,
        /// The error of the operating system.
        source: io::Error,
    },
    /// Cargo exited with an error.
    #[error("`cargo metadata` failed in {} ({status}): {stderr}", dir.display())]
    MetadataFailed {
        /// The directory of the run.
        dir: PathBuf,
        /// The exit status of cargo.
        status: String,
        /// What cargo wrote to its error stream.
        stderr: String,
    },
    /// The output of cargo is not JSON.
    #[error("the output of `cargo metadata` is not JSON: {0}")]
    NotJson(#[from] serde_json::Error),
    /// The JSON lacks a field that the guard reads.
    #[error("the output of `cargo metadata` has no usable `{field}`")]
    MissingField {
        /// The path of the field.
        field: String,
    },
    /// No workspace member is named [`OWNER`].
    #[error(
        "no workspace member is named `{OWNER}`: the guard has no owner, so it proves nothing"
    )]
    OwnerMissing,
}

/// One dependency on [`GUARDED_PACKAGE`] that a member must not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offender {
    /// Name of the member package.
    pub member: String,
    /// Path of the manifest of the member.
    pub manifest_path: PathBuf,
    /// `normal`, `dev`, `build`, or the raw kind that cargo reported.
    pub kind: String,
    /// The local name, when the dependency is renamed.
    pub rename: Option<String>,
    /// The `cfg` or triple of a `[target.*.dependencies]` table.
    pub target: Option<String>,
}

/// The verdict of [`audit`].
#[derive(Debug, Default)]
pub struct Report {
    offenders: Vec<Offender>,
    members_examined: usize,
}

impl Report {
    /// The offenders, in the order of the members.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// How many workspace members the audit read.
    #[must_use]
    pub fn members_examined(&self) -> usize {
        self.members_examined
    }

    /// True when no member but the owner depends on the guarded package.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_compliant() {
            return write!(
                f,
                "no member but `{OWNER}` depends on `{GUARDED_PACKAGE}` ({} members read)",
                self.members_examined
            );
        }
        writeln!(
            f,
            "{} dependencies on `{GUARDED_PACKAGE}` outside `{OWNER}`:",
            self.offenders.len()
        )?;
        for offender in &self.offenders {
            write!(
                f,
                "  {} ({}): {} dependency",
                offender.member,
                offender.manifest_path.display(),
                offender.kind
            )?;
            if let Some(rename) = &offender.rename {
                write!(f, ", renamed `{rename}`")?;
            }
            if let Some(target) = &offender.target {
                write!(f, ", target `{target}`")?;
            }
            writeln!(f)?;
        }
        write!(
            f,
            "Fix: remove `{GUARDED_PACKAGE}` from each manifest above. Depend on `{OWNER}` \
             and reach the clipboard through `{OWNER}::Clipboard`."
        )
    }
}

/// The program that starts cargo.
fn cargo_program() -> OsString {
    env::var_os(CARGO_ENV).unwrap_or_else(|| OsString::from(CARGO_FALLBACK))
}

/// Start cargo in `dir` with a scrubbed environment and collect its output.
fn run(program: &OsStr, dir: &Path) -> Result<Output, ArboardWallError> {
    let mut command = childenv::command(program);
    command.current_dir(dir).args(METADATA_ARGS);
    for name in SCRUBBED_VARS {
        command.env_remove(name);
    }
    for (name, _) in env::vars_os() {
        if name.to_string_lossy().starts_with(SCRUBBED_PREFIX) {
            command.env_remove(&name);
        }
    }
    command.output().map_err(|source| ArboardWallError::Spawn {
        program: program.to_string_lossy().into_owned(),
        dir: dir.to_path_buf(),
        source,
    })
}

/// A string field of `value`, or a refusal that names `path`.
fn str_field<'a>(value: &'a Value, key: &str, path: &str) -> Result<&'a str, ArboardWallError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ArboardWallError::MissingField {
            field: format!("{path}.{key}"),
        })
}

/// An array field of `value`, or a refusal that names `path`.
fn array_field<'a>(
    value: &'a Value,
    key: &str,
    path: &str,
) -> Result<&'a Vec<Value>, ArboardWallError> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| ArboardWallError::MissingField {
            field: format!("{path}.{key}"),
        })
}

/// An optional string field: absent or `null` is `None`, another type is a refusal.
fn optional_str(value: &Value, key: &str, path: &str) -> Result<Option<String>, ArboardWallError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(ArboardWallError::MissingField {
            field: format!("{path}.{key}"),
        }),
    }
}

/// The name of a dependency kind. `null` is a normal dependency.
fn kind_name(dependency: &Value) -> Result<String, ArboardWallError> {
    match dependency.get("kind") {
        None | Some(Value::Null) => Ok("normal".to_owned()),
        Some(Value::String(kind)) => Ok(kind.clone()),
        Some(_) => Err(ArboardWallError::MissingField {
            field: "packages[].dependencies[].kind".to_owned(),
        }),
    }
}

/// Read the metadata JSON in `text` and build the report.
fn report_from(text: &[u8]) -> Result<Report, ArboardWallError> {
    let root: Value = serde_json::from_slice(text)?;
    let packages = array_field(&root, "packages", "metadata")?;
    let members = array_field(&root, "workspace_members", "metadata")?;
    let member_ids: Vec<&str> = members.iter().filter_map(Value::as_str).collect();
    if member_ids.len() != members.len() {
        return Err(ArboardWallError::MissingField {
            field: "metadata.workspace_members[]".to_owned(),
        });
    }

    let mut report = Report::default();
    let mut owner_found = false;
    for package in packages {
        let id = str_field(package, "id", "packages[]")?;
        if !member_ids.contains(&id) {
            continue;
        }
        let name = str_field(package, "name", "packages[]")?;
        let manifest_path = str_field(package, "manifest_path", "packages[]")?;
        let dependencies = array_field(package, "dependencies", "packages[]")?;
        report.members_examined += 1;
        if name == OWNER {
            owner_found = true;
            continue;
        }
        for dependency in dependencies {
            let package_name = str_field(dependency, "name", "packages[].dependencies[]")?;
            if package_name != GUARDED_PACKAGE {
                continue;
            }
            report.offenders.push(Offender {
                member: name.to_owned(),
                manifest_path: PathBuf::from(manifest_path),
                kind: kind_name(dependency)?,
                rename: optional_str(dependency, "rename", "packages[].dependencies[]")?,
                target: optional_str(dependency, "target", "packages[].dependencies[]")?,
            });
        }
    }
    if !owner_found {
        return Err(ArboardWallError::OwnerMissing);
    }
    Ok(report)
}

/// Audit the workspace at `repo_root`.
///
/// # Errors
///
/// Returns an error when cargo cannot start or fails, when its output is not
/// the expected JSON, or when no workspace member is named [`OWNER`]. An error
/// is never a clean report.
pub fn audit(repo_root: &Path) -> Result<Report, ArboardWallError> {
    let program = cargo_program();
    let output = run(&program, repo_root)?;
    if !output.status.success() {
        return Err(ArboardWallError::MetadataFailed {
            dir: repo_root.to_path_buf(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    report_from(&output.stdout)
}
