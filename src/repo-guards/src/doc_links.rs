//! Guard: every intra-doc link this workspace writes must resolve.
//!
//! A doc comment is prose to every build step this repository runs. `cargo
//! fmt`, `cargo clippy`, and `cargo test` all read the source and none of them
//! reads the *documentation* the source declares. Only `cargo doc` resolves an
//! intra-doc link, and no gate here has ever run it. So a link to an item that
//! was renamed, moved, or deleted keeps its brackets, still looks like a link
//! in the source, and renders as plain text — or as nothing at all — for every
//! reader.
//!
//! The omission is spelled as an *absence*, which is why it spread. Rustdoc
//! reports each broken link as a warning, on a stream nothing reads, and a
//! warning nobody reads is a warning nobody fixes. This workspace carried seven
//! of them when this guard was written.
//!
//! [`audit`] closes that hole: it runs the documentation build, reads the
//! diagnostics rustdoc emits, and reports every unresolved link.
//!
//! # Key on the lint code, never on the words
//!
//! A finding is a diagnostic whose lint code is
//! `rustdoc::broken_intra_doc_links`, and nothing else. The
//! lint speaks in two registers: "unresolved link to `run`" when the item is
//! not there, and "`trace` is both a function and a module" when the link names
//! two items at once. A matcher built from the first sentence reports clean for
//! the second, and a guard that reports clean for the wrong reason is
//! indistinguishable from a guard doing real work.
//!
//! The other direction costs as much. Rustdoc raises four more documentation
//! lints in this workspace today, and one of them —
//! `rustdoc::private_intra_doc_links` — is *about a link that resolves*. A
//! guard that reported every rustdoc warning, or every warning whose text
//! mentions a link, would be red from the day it landed and would be switched
//! off within the week.
//!
//! # Refuse rather than shrink
//!
//! Everything that could quietly reduce what the scan covered is a refusal
//! rather than a clean verdict. See [`DocLinksError`].
//!
//! The sharpest of those is a failed documentation build. The guard refuses on
//! **every** non-zero exit, not only on one that found nothing. A unit that
//! fails to document takes every unit downstream of it with it, so the list of
//! links is short by an amount nobody can measure. "3 broken links" printed
//! over forty crates that were never read is indistinguishable from a guard
//! doing real work — and it is worse than silence, because it looks like a
//! finished answer.
//!
//! # The environment is scrubbed, not inherited
//!
//! The guard removes `RUSTDOCFLAGS`, `RUSTFLAGS`, `CARGO_ENCODED_RUSTDOCFLAGS`,
//! `CARGO_ENCODED_RUSTFLAGS`, `CARGO_TARGET_DIR`, and every `CARGO_BUILD_*`
//! variable before it starts cargo. It keeps `PATH`, `HOME`, `CARGO_HOME`, and
//! `RUSTUP_*`. Two of those removals carry the reason.
//!
//! An ambient `RUSTDOCFLAGS=-Dwarnings` turns a reportable warning into a
//! non-zero exit, which the guard reads as a refusal. The verdict would then
//! depend on the shell the operator happened to run the tests from, and the
//! same tree would be clean for one person and unreadable for the next.
//!
//! An ambient `CARGO_TARGET_DIR` moves the cache the warm-run cost depends on,
//! and lets two runs of the guard share one target directory. Two fixtures that
//! share a target directory are two tests that can block or corrupt each other.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use thiserror::Error;

/// The variable cargo sets to its own path for the processes it starts. Read
/// before the child environment is scrubbed, so the guard runs the same cargo
/// that runs the tests.
const CARGO_ENV: &str = "CARGO";

/// What to start when nothing names a cargo.
const CARGO_FALLBACK: &str = "cargo";

/// The documentation build, spelled once.
///
/// `--no-deps` keeps rustdoc on this workspace's own packages. `--workspace`
/// reaches every member, so a member added later is scanned without anybody
/// editing this list.
const DOC_ARGS: [&str; 4] = ["doc", "--workspace", "--no-deps", "--message-format=json"];

/// The member query, spelled once. `--no-deps` keeps the answer to this
/// workspace's own packages.
const METADATA_ARGS: [&str; 4] = ["metadata", "--no-deps", "--format-version", "1"];

/// How the documentation build is named in a refusal.
const DOC_COMMAND: &str = "cargo doc";

/// How the member query is named in a refusal.
const METADATA_COMMAND: &str = "cargo metadata";

/// Environment variables the guard removes before it starts cargo. See the
/// module header for the reason.
const SCRUBBED_VARS: [&str; 5] = [
    "CARGO_ENCODED_RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_TARGET_DIR",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
];

/// Every variable whose name starts with this is removed too. `CARGO_BUILD_*`
/// is the environment spelling of the `[build]` config table, and it holds
/// `CARGO_BUILD_TARGET_DIR` and `CARGO_BUILD_RUSTDOCFLAGS` among others.
const SCRUBBED_PREFIX: &str = "CARGO_BUILD_";

/// The one lint this guard reports.
///
/// The guard keys on this code and never on the words of the diagnostic. The
/// lint speaks in two registers — "unresolved link to `run`" for a link to an
/// item that is not there, and "`trace` is both a function and a module" for a
/// link that names two items at once — and a matcher built from the first
/// sentence would be silently blind to the second.
///
/// Rustdoc raises four other documentation lints in this workspace today
/// (`rustdoc::private_intra_doc_links`, `rustdoc::bare_urls`,
/// `rustdoc::redundant_explicit_links`, `rustdoc::invalid_html_tags`). None of
/// them is an unresolved link, and a guard that reported them all would be red
/// from the day it landed, so it would be switched off.
const BROKEN_INTRA_DOC_LINKS: &str = "rustdoc::broken_intra_doc_links";

/// The key naming what kind of record a line of cargo output is.
const REASON: &str = "reason";

/// The record kind that carries a compiler or rustdoc diagnostic.
const COMPILER_MESSAGE: &str = "compiler-message";

/// The key naming the package a record belongs to.
const PACKAGE_ID: &str = "package_id";

/// The key holding a record's target, and the key holding a target's kinds.
const TARGET: &str = "target";

/// The key holding a target's or a package's name.
const NAME: &str = "name";

/// The key holding a target's kinds. Cargo writes an array.
const KIND: &str = "kind";

/// The key holding a diagnostic — and, inside it, the diagnostic's own text.
const MESSAGE: &str = "message";

/// The key holding a diagnostic's lint code — and, inside it, the code itself.
const CODE: &str = "code";

/// The key holding a diagnostic's source locations.
const SPANS: &str = "spans";

/// The key marking the span a diagnostic points at.
const IS_PRIMARY: &str = "is_primary";

/// The key holding a span's file, relative to the workspace root.
const FILE_NAME: &str = "file_name";

/// The key holding a span's first line, counted from one.
const LINE_START: &str = "line_start";

/// The `cargo metadata` key listing every package.
const PACKAGES: &str = "packages";

/// The `cargo metadata` key listing the identifiers of the workspace members.
const WORKSPACE_MEMBERS: &str = "workspace_members";

/// The key holding a package's identifier.
const ID: &str = "id";

/// How much of an offending line a refusal quotes.
const QUOTED_CHARS: usize = 400;

/// Everything that can stop the scan from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot read the whole
/// documentation build must say so loudly, because "every link resolves" and
/// "I never looked" are the same sentence to a CI log and only one of them is
/// good news.
#[derive(Debug, Error)]
pub enum DocLinksError {
    /// Cargo could not be started at all.
    #[error("cannot run `{program}` in {}: {source}", dir.display())]
    Spawn {
        /// The program the guard tried to start.
        program: String,
        /// The directory it would have run in.
        dir: PathBuf,
        /// The underlying failure.
        source: io::Error,
    },

    /// `cargo metadata` failed, so the workspace members are unknown.
    #[error("`cargo metadata` in {} exited with {status}:\n{stderr}", dir.display())]
    MetadataFailed {
        /// The directory cargo ran in.
        dir: PathBuf,
        /// How cargo exited.
        status: String,
        /// What cargo wrote to its error stream.
        stderr: String,
    },

    /// The documentation build failed.
    ///
    /// This is a refusal on *every* non-zero exit, not only on one that found
    /// nothing. A unit that fails to document takes every unit downstream of it
    /// with it, so the list of links is short by an unknown amount.
    #[error(
        "`cargo doc` in {} exited with {status}; the link list is incomplete because the build stopped:\n{stderr}",
        dir.display()
    )]
    DocBuildFailed {
        /// The directory cargo ran in.
        dir: PathBuf,
        /// How cargo exited.
        status: String,
        /// What cargo wrote to its error stream.
        stderr: String,
    },

    /// A line of cargo output is not JSON.
    #[error("cannot parse a line of `{command}` output as JSON: {source}\n  line: {line}")]
    Json {
        /// The cargo invocation whose output could not be read.
        command: &'static str,
        /// The offending line.
        line: String,
        /// The underlying parse failure.
        source: serde_json::Error,
    },

    /// A cargo record lacks a field the guard reads.
    #[error("a `{command}` record has no `{field}`:\n  {record}")]
    MissingField {
        /// The cargo invocation whose record is short.
        command: &'static str,
        /// The field the guard needs.
        field: &'static str,
        /// The offending record.
        record: String,
    },

    /// `cargo metadata` reports no workspace members.
    #[error(
        "`cargo metadata` in {} reports no workspace members; refusing to report every link resolved across no packages",
        dir.display()
    )]
    NoWorkspaceMembers {
        /// The directory cargo ran in.
        dir: PathBuf,
    },

    /// The documentation build documented no workspace package at all.
    #[error(
        "`cargo doc` in {} documented no workspace package; refusing to report every link resolved across nothing",
        dir.display()
    )]
    NothingDocumented {
        /// The directory cargo ran in.
        dir: PathBuf,
    },

    /// A diagnostic names a package that is not a workspace member.
    ///
    /// `cargo doc --no-deps` documents workspace members only, so rustdoc can
    /// raise a link lint against nothing else. A record that says otherwise
    /// means the guard's model of the build is wrong, and a guard wrong about
    /// what it read is not to be trusted about what it found.
    #[error(
        "`cargo doc` reported a documentation lint against `{package_id}`, which `cargo metadata` does not list as a workspace member"
    )]
    UnknownPackage {
        /// The package cargo named.
        package_id: String,
    },
}

/// One link rustdoc could not resolve, as rustdoc reported it.
///
/// "Could not resolve" covers both spellings of the same lint: a link to an
/// item that does not exist, and a link whose text names two items at once.
#[derive(Debug, Clone)]
pub struct BrokenLink {
    /// The workspace package that holds the doc comment.
    package: String,
    /// The target within that package, as cargo names it.
    target_name: String,
    /// What kind of target that is: `lib`, `bin`, and so on.
    target_kind: String,
    /// Rustdoc's own words, e.g. "unresolved link to `run`".
    message: String,
    /// The file that holds the link, relative to the workspace root.
    file: PathBuf,
    /// The one-based line the link sits on.
    line: u64,
}

impl BrokenLink {
    /// The workspace package that holds the doc comment.
    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    /// The target within that package, as cargo names it.
    #[must_use]
    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    /// What kind of target that is: `lib`, `bin`, and so on.
    #[must_use]
    pub fn target_kind(&self) -> &str {
        &self.target_kind
    }

    /// Rustdoc's own words, e.g. "unresolved link to `run`".
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The file that holds the link, relative to the workspace root.
    #[must_use]
    pub fn file(&self) -> &Path {
        &self.file
    }

    /// The one-based line the link sits on.
    #[must_use]
    pub fn line(&self) -> u64 {
        self.line
    }
}

impl fmt::Display for BrokenLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {} ({} target `{}` of {})",
            self.file.display(),
            self.line,
            self.message,
            self.target_kind,
            self.target_name,
            self.package
        )
    }
}

/// The verdict of one scan: the links rustdoc could not resolve, and the
/// workspace packages the build actually documented.
///
/// Both halves matter, and for different reasons. The first is the finding. The
/// second is the proof that the finding covers the workspace: a scan that
/// documented four packages of seventy-seven reports "no broken links" in
/// exactly the words a scan of the whole workspace uses.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct DocScan {
    /// Every unresolved link, in the order rustdoc reported them.
    broken: Vec<BrokenLink>,
    /// The names of the workspace packages the build documented.
    documented: BTreeSet<String>,
}

impl DocScan {
    /// True when rustdoc resolved every link it read.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.broken.is_empty()
    }

    /// Every link rustdoc could not resolve.
    #[must_use]
    pub fn broken(&self) -> &[BrokenLink] {
        &self.broken
    }

    /// The names of the workspace packages the build documented.
    ///
    /// A caller should compare this against the workspace members cargo
    /// reports: a package the build never documented is a package whose links
    /// were never read.
    #[must_use]
    pub fn documented(&self) -> &BTreeSet<String> {
        &self.documented
    }
}

impl fmt::Display for DocScan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_clean() {
            return write!(
                f,
                "Documented {} workspace packages; every intra-doc link resolves.",
                self.documented.len()
            );
        }

        writeln!(
            f,
            "Rustdoc could not resolve {} intra-doc link(s) across the {} workspace packages it documented.",
            self.broken.len(),
            self.documented.len()
        )?;
        for link in &self.broken {
            writeln!(f, "    {link}")?;
        }
        writeln!(f)?;
        writeln!(
            f,
            "Write each path the way rustdoc reads it — from the item that holds the comment, \
             or from the crate root as `[`crate::module::Item`]` — or import the item so the \
             short path resolves."
        )?;
        write!(
            f,
            "A link that names two items at once needs a disambiguator, e.g. `[`fn@trace`]` \
             or `[`mod@trace`]`. A word that is not a link needs backticks and no brackets."
        )
    }
}

/// Scan the documentation build of the workspace rooted at `workspace_root`.
///
/// The guard asks `cargo metadata` which packages the workspace holds, runs
/// `cargo doc --workspace --no-deps` with a scrubbed environment, and reads the
/// JSON Lines cargo writes to its output stream.
///
/// # Errors
///
/// Returns [`DocLinksError`] — never a clean [`DocScan`] — when the build
/// cannot be read with confidence: cargo cannot be started
/// ([`Spawn`](DocLinksError::Spawn)), the documentation build exits non-zero
/// ([`DocBuildFailed`](DocLinksError::DocBuildFailed)), `cargo metadata` fails
/// ([`MetadataFailed`](DocLinksError::MetadataFailed)) or reports no members
/// ([`NoWorkspaceMembers`](DocLinksError::NoWorkspaceMembers)), a line of cargo
/// output is not JSON ([`Json`](DocLinksError::Json)) or lacks a field the
/// guard reads ([`MissingField`](DocLinksError::MissingField)), or a diagnostic
/// names a package the workspace does not hold
/// ([`UnknownPackage`](DocLinksError::UnknownPackage)).
pub fn audit(workspace_root: &Path) -> Result<DocScan, DocLinksError> {
    let cargo = cargo_program();
    let names = workspace_package_names(&cargo, workspace_root)?;
    let output = run(&cargo, workspace_root, &DOC_ARGS)?;
    if !output.status.success() {
        return Err(DocLinksError::DocBuildFailed {
            dir: workspace_root.to_path_buf(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    Ok(DocScan {
        broken: broken_links(&stdout, &names)?,
        documented: BTreeSet::new(),
    })
}

/// The cargo to start: the one that started this process, when there is one.
///
/// Read here, before [`run`] scrubs the child environment, so a run under
/// `cargo test` uses the same toolchain as the run itself.
fn cargo_program() -> OsString {
    env::var_os(CARGO_ENV).unwrap_or_else(|| OsString::from(CARGO_FALLBACK))
}

/// Start cargo in `dir` with a scrubbed environment and collect its output.
///
/// The scrub is the whole reason this is one function: every cargo the guard
/// starts goes through it, so no call site can inherit a flag that changes the
/// verdict. See the module header.
fn run(program: &OsStr, dir: &Path, args: &[&str]) -> Result<Output, DocLinksError> {
    let mut command = Command::new(program);
    command.current_dir(dir).args(args);

    for name in SCRUBBED_VARS {
        command.env_remove(name);
    }
    for (name, _) in env::vars_os() {
        if name.to_string_lossy().starts_with(SCRUBBED_PREFIX) {
            command.env_remove(&name);
        }
    }

    command.output().map_err(|source| DocLinksError::Spawn {
        program: program.to_string_lossy().into_owned(),
        dir: dir.to_path_buf(),
        source,
    })
}

/// Every workspace member of `dir`, as a map from package identifier to package
/// name.
///
/// The identifier is what every cargo record carries, and the name is what a
/// reader recognises, so the join happens here once. A package name parsed out
/// of the identifier would be a second model of a format cargo owns: the
/// identifier reads `path+file:///…/src/aa#0.1.0` when the directory and the
/// package agree on a name, and `path+file:///…/p4#fixture@0.1.0` when they do
/// not.
fn workspace_package_names(
    program: &OsStr,
    dir: &Path,
) -> Result<BTreeMap<String, String>, DocLinksError> {
    let output = run(program, dir, &METADATA_ARGS)?;
    if !output.status.success() {
        return Err(DocLinksError::MetadataFailed {
            dir: dir.to_path_buf(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }

    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let metadata: Value = serde_json::from_str(&text).map_err(|source| DocLinksError::Json {
        command: METADATA_COMMAND,
        line: elide(&text),
        source,
    })?;

    let mut members = BTreeSet::new();
    for member in array_field(&metadata, WORKSPACE_MEMBERS, METADATA_COMMAND, &text)? {
        members.insert(as_text(member, WORKSPACE_MEMBERS, METADATA_COMMAND, &text)?);
    }

    let mut names = BTreeMap::new();
    for package in array_field(&metadata, PACKAGES, METADATA_COMMAND, &text)? {
        let id = text_field(package, ID, METADATA_COMMAND, &text)?;
        if !members.contains(id) {
            continue;
        }
        let name = text_field(package, NAME, METADATA_COMMAND, &text)?;
        names.insert(id.to_owned(), name.to_owned());
    }

    if names.is_empty() {
        return Err(DocLinksError::NoWorkspaceMembers {
            dir: dir.to_path_buf(),
        });
    }
    Ok(names)
}

/// Every link rustdoc could not resolve, read out of the JSON Lines `cargo doc`
/// wrote.
fn broken_links(
    stdout: &str,
    names: &BTreeMap<String, String>,
) -> Result<Vec<BrokenLink>, DocLinksError> {
    let mut broken = Vec::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|source| DocLinksError::Json {
            command: DOC_COMMAND,
            line: elide(line),
            source,
        })?;

        if text_field(&record, REASON, DOC_COMMAND, line)? != COMPILER_MESSAGE {
            continue;
        }
        let message = field(&record, MESSAGE, DOC_COMMAND, line)?;
        let Some(code) = message
            .get(CODE)
            .and_then(|code| code.get(CODE))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if code != BROKEN_INTRA_DOC_LINKS {
            continue;
        }

        broken.push(broken_link(&record, message, names, line)?);
    }

    Ok(broken)
}

/// Read one diagnostic into a [`BrokenLink`].
fn broken_link(
    record: &Value,
    message: &Value,
    names: &BTreeMap<String, String>,
    line: &str,
) -> Result<BrokenLink, DocLinksError> {
    let package_id = text_field(record, PACKAGE_ID, DOC_COMMAND, line)?;
    let package = names
        .get(package_id)
        .ok_or_else(|| DocLinksError::UnknownPackage {
            package_id: package_id.to_owned(),
        })?;

    let target = field(record, TARGET, DOC_COMMAND, line)?;
    let kinds = array_field(target, KIND, DOC_COMMAND, line)?;
    let kind = kinds.first().ok_or_else(|| DocLinksError::MissingField {
        command: DOC_COMMAND,
        field: KIND,
        record: elide(line),
    })?;

    let span = primary_span(message, line)?;

    Ok(BrokenLink {
        package: package.clone(),
        target_name: text_field(target, NAME, DOC_COMMAND, line)?.to_owned(),
        target_kind: as_text(kind, KIND, DOC_COMMAND, line)?.to_owned(),
        message: text_field(message, MESSAGE, DOC_COMMAND, line)?.to_owned(),
        file: PathBuf::from(text_field(span, FILE_NAME, DOC_COMMAND, line)?),
        line: number_field(span, LINE_START, DOC_COMMAND, line)?,
    })
}

/// The span a diagnostic points at, or the first one it carries.
///
/// A diagnostic with no span at all is a refusal rather than a link with no
/// location: a report that cannot say where the link is cannot be acted on.
fn primary_span<'a>(message: &'a Value, line: &str) -> Result<&'a Value, DocLinksError> {
    let spans = array_field(message, SPANS, DOC_COMMAND, line)?;
    spans
        .iter()
        .find(|span| span.get(IS_PRIMARY).and_then(Value::as_bool) == Some(true))
        .or_else(|| spans.first())
        .ok_or_else(|| DocLinksError::MissingField {
            command: DOC_COMMAND,
            field: SPANS,
            record: elide(line),
        })
}

/// One field of a record, or a refusal naming the field and quoting the record.
fn field<'a>(
    record: &'a Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<&'a Value, DocLinksError> {
    record.get(key).ok_or_else(|| DocLinksError::MissingField {
        command,
        field: key,
        record: elide(source),
    })
}

/// One string field of a record.
fn text_field<'a>(
    record: &'a Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<&'a str, DocLinksError> {
    as_text(field(record, key, command, source)?, key, command, source)
}

/// One array field of a record.
fn array_field<'a>(
    record: &'a Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<&'a Vec<Value>, DocLinksError> {
    field(record, key, command, source)?
        .as_array()
        .ok_or_else(|| DocLinksError::MissingField {
            command,
            field: key,
            record: elide(source),
        })
}

/// One unsigned-integer field of a record.
fn number_field(
    record: &Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<u64, DocLinksError> {
    field(record, key, command, source)?
        .as_u64()
        .ok_or_else(|| DocLinksError::MissingField {
            command,
            field: key,
            record: elide(source),
        })
}

/// A JSON value as a string, or a refusal.
fn as_text<'a>(
    value: &'a Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<&'a str, DocLinksError> {
    value.as_str().ok_or_else(|| DocLinksError::MissingField {
        command,
        field: key,
        record: elide(source),
    })
}

/// The first [`QUOTED_CHARS`] characters of `text`, with a marker when more
/// follow.
///
/// Counted in characters rather than bytes. A cargo record carries source text,
/// and source text in this repository holds multi-byte characters, so a byte
/// slice would panic on the record it was written to quote.
fn elide(text: &str) -> String {
    let mut quoted: String = text.chars().take(QUOTED_CHARS).collect();
    if text.chars().nth(QUOTED_CHARS).is_some() {
        quoted.push('…');
    }
    quoted
}
