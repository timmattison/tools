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
//! # A documented unit is not a compiled unit
//!
//! The scan reports which targets the build documented, and a caller compares
//! that set against the documentable targets cargo reports. A target the build
//! never reached raises no diagnostics, so its links are called resolved for
//! the same reason a target with no links is — and the verdict says "clean" in
//! both cases, in the same words.
//!
//! A unit was documented when one of the files it produced lies under
//! `<target_directory>/doc/`. That test is load-bearing, because
//! `cargo doc --no-deps` still *compiles* every member another member depends
//! on, and each of those compilations reports an artifact of its own — with
//! `.rmeta` files under `<target_directory>/debug/deps/`. The workspace pass
//! over this repository reports 758 artifacts across 672 packages, of which 77
//! are documentation. A count that skipped the test would make the parity check
//! pass for the wrong reason.
//!
//! The target directory comes from `cargo metadata`, never from
//! `<root>/target`. A `[build] target-dir` in any config file cargo reads moves
//! it, and a guard that guessed would then find no documented unit anywhere.
//!
//! # The package is the wrong grain
//!
//! Those 77 are 77 *targets*, not 77 packages, and the difference is a whole
//! class of unread code. `cargo doc --workspace` names no target, so cargo
//! applies its default target filter — and that filter drops a binary whose
//! name equals its package's library name, silently, with nothing on stderr and
//! a zero exit. This repository holds 87 documentable targets and ten binaries
//! in exactly that shape, two of them carrying intra-doc links.
//!
//! A parity check keyed on the package cannot see any of it. The library of
//! such a package *is* documented, so the package is in the documented set, so
//! the binary beside it reads as covered. The scan then prints a clean verdict
//! over targets nothing ever read: the same false green, one level down.
//!
//! So [`audit`] asks `cargo metadata` which targets are documentable — cargo
//! answers that from the manifest, in a boolean `doc` field, rather than the
//! guard modelling it — and then documents whatever the workspace pass left
//! out, one target at a time. A target still unread after that is
//! [`TargetsUnread`](DocLinksError::TargetsUnread), never a clean verdict.
//!
//! The reason cargo drops the binary is that the two write the same page:
//! `target/doc/<name>/index.html` is the output of both a library and a binary
//! of that name (rust-lang/cargo#6313). Documenting the binary anyway therefore
//! overwrites the library's page with the binary's. The guard reads
//! diagnostics rather than pages, so that costs it nothing — but it is why
//! neither unit is ever fresh, which the cost section prices.
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
//! # What the scan costs, and where it runs
//!
//! Measured on this workspace: 77 members and 87 documentable targets, ten of
//! which the workspace pass leaves to a targeted pass.
//!
//! - Cold, with an empty target directory: about 74 seconds for the workspace
//!   pass. The target directory grows to about 741 MB, of which the rendered
//!   pages under `target/doc` are about 42 MB. The targeted passes compile
//!   nothing the workspace pass has not already compiled; they only document
//!   ten more binaries.
//! - Warm, with nothing changed: about 19 seconds — about 4 for the workspace
//!   pass and about 14 for the ten targeted passes.
//! - Warm, with one crate edited: one to three seconds more (1.5 s measured).
//!
//! Cargo *replays* the rustdoc diagnostics of a unit it does not rebuild: a
//! fresh unit reports `"fresh": true` and its warnings arrive all the same. So
//! the 77 targets the workspace pass reaches on its own cost almost nothing on
//! a second run, and the whole scan was under a second before the targeted
//! passes were added.
//!
//! Those ten are the exception, and the reason is the collision itself. A
//! library and a binary of the same name render to the same
//! `target/doc/<name>/index.html`, so each pass makes the other's unit stale
//! and neither is ever fresh. Ten libraries are re-documented by every
//! workspace pass and ten binaries by every targeted one. A workspace where no
//! binary shares a library's name runs no targeted pass and pays none of this.
//!
//! The guard therefore needs no gate of its own. It runs as a test, under the
//! `cargo test` the pre-commit hook already runs, and pays a warm build. A
//! fifth pre-commit gate would buy nothing and would add a step to maintain.
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

/// The cargo subcommand that renders documentation.
const DOC: &str = "doc";

/// The flag that reaches every workspace member.
const WORKSPACE_FLAG: &str = "--workspace";

/// The flag that reaches one package, named in the argument after it.
const PACKAGE_FLAG: &str = "-p";

/// The flag that keeps rustdoc on this workspace's own packages.
const NO_DEPS: &str = "--no-deps";

/// The flag that asks cargo for JSON Lines rather than prose.
const JSON_MESSAGES: &str = "--message-format=json";

/// The workspace documentation build, spelled once.
///
/// `--workspace` reaches every member, so a member added later is scanned
/// without anybody editing this list.
///
/// It does not reach every *target*. These arguments name no target, so cargo
/// applies its default target filter, and that filter drops a binary whose name
/// equals its package's library name. [`audit`] therefore follows this pass
/// with one [`target_args`] pass per target it left out. See the module header.
///
/// `--lib --bins` is not the shortcut it looks like. `--lib` overrides a
/// manifest `[lib] doc = false` — the one thing
/// [`NothingDocumented`](DocLinksError::NothingDocumented) exists to catch — and
/// it fails outright on a package that has no library at all, which most
/// members here are.
const DOC_ARGS: [&str; 4] = [DOC, WORKSPACE_FLAG, NO_DEPS, JSON_MESSAGES];

/// The flag that names a package's library. It carries no target name, because
/// a package has at most one library.
const LIB_FLAG: &str = "--lib";

/// Every target kind cargo builds out of a package's one library.
///
/// A library declared `crate-type = ["cdylib", "rlib"]` reports `cdylib` as its
/// first kind, and `--lib` is still how cargo is asked for it.
const LIB_KINDS: [&str; 6] = ["cdylib", "dylib", "lib", "proc-macro", "rlib", "staticlib"];

/// The flag that names one target, for each kind of target that has a name of
/// its own. A kind absent from this list cannot be asked for, so a target of
/// that kind the workspace pass missed is a refusal rather than a retry.
const NAMED_TARGET_FLAGS: [(&str, &str); 4] = [
    ("bench", "--bench"),
    ("bin", "--bin"),
    ("example", "--example"),
    ("test", "--test"),
];

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

/// The record kind that names the files one unit of the build produced.
const COMPILER_ARTIFACT: &str = "compiler-artifact";

/// The key naming the package a record belongs to.
const PACKAGE_ID: &str = "package_id";

/// The key holding a record's target, and the key holding a target's kinds.
const TARGET: &str = "target";

/// The key holding a target's or a package's name.
const NAME: &str = "name";

/// The key holding a target's kinds. Cargo writes an array.
const KIND: &str = "kind";

/// The `cargo metadata` key listing a package's targets.
const TARGETS: &str = "targets";

/// The key saying whether cargo documents a target.
///
/// Cargo answers this from the manifest, so reading it asks the toolchain which
/// targets are documentable rather than modelling the answer here. A list of
/// documentable kinds written in this file would part company with cargo on the
/// first kind nobody here thought of, and it would do so in the direction that
/// reports clean.
const DOC_FIELD: &str = "doc";

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

/// The key listing the files one unit of the build produced.
const FILENAMES: &str = "filenames";

/// The `cargo metadata` key naming the directory cargo builds into.
const TARGET_DIRECTORY: &str = "target_directory";

/// The subdirectory of the target directory that holds rendered documentation.
///
/// This one path separates a unit that was *documented* from a unit that was
/// merely *compiled*. See the module header.
const DOC_SUBDIR: &str = "doc";

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

    /// The documentation build documented no target at all.
    #[error(
        "`cargo doc` in {} documented no target; refusing to report every link resolved across nothing",
        dir.display()
    )]
    NothingDocumented {
        /// The directory cargo ran in.
        dir: PathBuf,
    },

    /// A target `cargo metadata` calls documentable was never documented.
    ///
    /// Cargo's default target filter drops a binary whose name equals its
    /// package's library name, and says nothing about it. [`audit`] documents
    /// each such target on its own afterwards; this refusal is what happens
    /// when one is *still* unread once that has run.
    ///
    /// It is the fail-closed direction, and it has to be. A target nothing
    /// documented raises no diagnostics, so its links come back resolved
    /// without ever having been read — in exactly the words a scan that read
    /// them would use.
    #[error(
        "`cargo doc` in {} never documented {} documentable target(s), so their intra-doc links were never read: {}",
        dir.display(),
        targets.len(),
        targets.join(", ")
    )]
    TargetsUnread {
        /// The directory cargo ran in.
        dir: PathBuf,
        /// The unread targets, each as [`DocTarget`] renders it.
        targets: Vec<String>,
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

/// One unit of documentation: one target of one workspace member.
///
/// This, rather than the package, is what the scan counts and what a caller
/// compares against `cargo metadata`. A package with a library and a binary is
/// two documentation units; they are reached separately, they fail separately,
/// and cargo's default target filter reaches one of them and not the other. See
/// the module header.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DocTarget {
    /// The workspace package the target belongs to.
    package: String,
    /// The target's own name, as cargo names it.
    name: String,
    /// What kind of target it is: `lib`, `bin`, and so on. Cargo writes an
    /// array of kinds; this is its first entry.
    kind: String,
}

impl DocTarget {
    /// The workspace package the target belongs to.
    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    /// The target's own name, as cargo names it.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What kind of target it is: `lib`, `bin`, and so on.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

impl fmt::Display for DocTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} target `{}` of {}",
            self.kind, self.name, self.package
        )
    }
}

/// One link rustdoc could not resolve, as rustdoc reported it.
///
/// "Could not resolve" covers both spellings of the same lint: a link to an
/// item that does not exist, and a link whose text names two items at once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BrokenLink {
    /// The target whose documentation holds the comment.
    target: DocTarget,
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
        self.target.package()
    }

    /// The target within that package, as cargo names it.
    #[must_use]
    pub fn target_name(&self) -> &str {
        self.target.name()
    }

    /// What kind of target that is: `lib`, `bin`, and so on.
    #[must_use]
    pub fn target_kind(&self) -> &str {
        self.target.kind()
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
            "{}:{}: {} ({})",
            self.file.display(),
            self.line,
            self.message,
            self.target
        )
    }
}

/// The verdict of one scan: the links rustdoc could not resolve, and the
/// targets the build actually documented.
///
/// Both halves matter, and for different reasons. The first is the finding. The
/// second is the proof that the finding covers the workspace: a scan that
/// documented four targets of eighty-seven reports "no broken links" in exactly
/// the words a scan of the whole workspace uses.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct DocScan {
    /// Every unresolved link, in the order rustdoc reported them.
    broken: Vec<BrokenLink>,
    /// The targets the build documented.
    documented: BTreeSet<DocTarget>,
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

    /// The targets the build documented.
    ///
    /// A caller should compare this against the documentable targets cargo
    /// reports: a target the build never documented is a target whose links
    /// were never read. The comparison is per target and not per package,
    /// because a package's library being documented says nothing about the
    /// binary beside it. See the module header.
    #[must_use]
    pub fn documented(&self) -> &BTreeSet<DocTarget> {
        &self.documented
    }

    /// Take on everything another pass over the same workspace found.
    ///
    /// A link both passes reported is one finding, not two. A pass that names
    /// one target still carries every unit that target depends on, and cargo
    /// replays the diagnostics of a unit it does not rebuild, so the same link
    /// can arrive twice. Deduplication belongs here, where both halves are in
    /// hand, rather than at the point where a report is printed.
    fn merge(&mut self, other: Self) {
        let mut seen: BTreeSet<BrokenLink> = self.broken.iter().cloned().collect();
        for link in other.broken {
            if seen.insert(link.clone()) {
                self.broken.push(link);
            }
        }
        self.documented.extend(other.documented);
    }
}

impl fmt::Display for DocScan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_clean() {
            return write!(
                f,
                "Documented {} workspace targets; every intra-doc link resolves.",
                self.documented.len()
            );
        }

        writeln!(
            f,
            "Rustdoc could not resolve {} intra-doc link(s) across the {} workspace targets it documented.",
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
/// The guard asks `cargo metadata` which packages the workspace holds and which
/// of their targets cargo documents, runs `cargo doc --workspace --no-deps`
/// with a scrubbed environment, and reads the JSON Lines cargo writes to its
/// output stream. It then documents every documentable target that pass left
/// out, one at a time, because cargo's default target filter silently drops a
/// binary whose name equals its package's library name. See the module header.
///
/// # Errors
///
/// Returns [`DocLinksError`] — never a clean [`DocScan`] — when the build
/// cannot be read with confidence: cargo cannot be started
/// ([`Spawn`](DocLinksError::Spawn)), a documentation build exits non-zero
/// ([`DocBuildFailed`](DocLinksError::DocBuildFailed)), `cargo metadata` fails
/// ([`MetadataFailed`](DocLinksError::MetadataFailed)) or reports no members
/// ([`NoWorkspaceMembers`](DocLinksError::NoWorkspaceMembers)), a line of cargo
/// output is not JSON ([`Json`](DocLinksError::Json)) or lacks a field the
/// guard reads ([`MissingField`](DocLinksError::MissingField)), the build
/// documents no target at all
/// ([`NothingDocumented`](DocLinksError::NothingDocumented)), a documentable
/// target is still unread once the targeted passes have run
/// ([`TargetsUnread`](DocLinksError::TargetsUnread)), or a diagnostic names a
/// package the workspace does not hold
/// ([`UnknownPackage`](DocLinksError::UnknownPackage)).
pub fn audit(workspace_root: &Path) -> Result<DocScan, DocLinksError> {
    let cargo = cargo_program();
    let workspace = workspace(&cargo, workspace_root)?;

    let mut scan = doc_pass(&cargo, workspace_root, &workspace, &DOC_ARGS)?;
    for target in unread(&workspace.documentable, &scan.documented) {
        // A pass that names one target documents everything that target's own
        // unit graph reaches, which can include another target this list still
        // calls unread. Asking again would be a second cargo start for work
        // already done.
        if scan.documented.contains(&target) {
            continue;
        }
        let Some(args) = target_args(&target) else {
            continue;
        };
        let pass = doc_pass(&cargo, workspace_root, &workspace, &args)?;
        scan.merge(pass);
    }

    if scan.documented.is_empty() {
        return Err(DocLinksError::NothingDocumented {
            dir: workspace_root.to_path_buf(),
        });
    }

    let still_unread = unread(&workspace.documentable, &scan.documented);
    if !still_unread.is_empty() {
        return Err(DocLinksError::TargetsUnread {
            dir: workspace_root.to_path_buf(),
            targets: still_unread.iter().map(ToString::to_string).collect(),
        });
    }
    Ok(scan)
}

/// Run one documentation build and read what it wrote.
///
/// Every pass goes through here, so the refusal on a non-zero exit covers the
/// targeted passes exactly as it covers the workspace pass. A targeted pass
/// that fails takes its target's links with it, and a list of links short by an
/// unmeasurable amount is worse than silence.
fn doc_pass(
    cargo: &OsStr,
    dir: &Path,
    workspace: &Workspace,
    args: &[&str],
) -> Result<DocScan, DocLinksError> {
    let output = run(cargo, dir, args)?;
    if !output.status.success() {
        return Err(DocLinksError::DocBuildFailed {
            dir: dir.to_path_buf(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    read_build(&String::from_utf8_lossy(&output.stdout), workspace)
}

/// The documentable targets no pass has documented yet.
///
/// Owned rather than borrowed, so the caller can document them while holding
/// the scan they came out of. Empty on a workspace where no target name
/// collides, which is why the targeted passes cost nothing there.
fn unread(documentable: &BTreeSet<DocTarget>, documented: &BTreeSet<DocTarget>) -> Vec<DocTarget> {
    documentable.difference(documented).cloned().collect()
}

/// The documentation build that reaches exactly one target, or `None` when the
/// guard has no way to name a target of that kind.
///
/// `None` is not a pass: the target stays unread, and [`audit`] refuses with
/// [`TargetsUnread`](DocLinksError::TargetsUnread) naming it. A kind nobody
/// here anticipated therefore arrives as a refusal rather than as a gap.
fn target_args(target: &DocTarget) -> Option<Vec<&str>> {
    let mut args = vec![DOC, PACKAGE_FLAG, target.package.as_str()];
    if LIB_KINDS.contains(&target.kind.as_str()) {
        args.push(LIB_FLAG);
    } else {
        let (_, flag) = NAMED_TARGET_FLAGS
            .iter()
            .find(|(kind, _)| *kind == target.kind)?;
        args.push(flag);
        args.push(target.name.as_str());
    }
    args.push(NO_DEPS);
    args.push(JSON_MESSAGES);
    Some(args)
}

/// What `cargo metadata` tells the guard before the documentation build starts.
struct Workspace {
    /// Package identifier to package name, for the workspace members only.
    names: BTreeMap<String, String>,
    /// Every target of a workspace member that cargo says it documents.
    documentable: BTreeSet<DocTarget>,
    /// The directory a documented unit writes its pages into.
    doc_dir: PathBuf,
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

/// Every workspace member of `dir`: its identifier, its name, and the targets
/// cargo documents for it.
///
/// The identifier is what every cargo record carries, and the name is what a
/// reader recognises, so the join happens here once. A package name parsed out
/// of the identifier would be a second model of a format cargo owns: the
/// identifier reads `path+file:///…/src/aa#0.1.0` when the directory and the
/// package agree on a name, and `path+file:///…/p4#fixture@0.1.0` when they do
/// not.
///
/// Which targets are documentable is cargo's answer too, read from the
/// [`DOC_FIELD`] of each target rather than decided here from its kind.
fn workspace(program: &OsStr, dir: &Path) -> Result<Workspace, DocLinksError> {
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
    let mut documentable = BTreeSet::new();
    for package in array_field(&metadata, PACKAGES, METADATA_COMMAND, &text)? {
        let id = text_field(package, ID, METADATA_COMMAND, &text)?;
        if !members.contains(id) {
            continue;
        }
        let name = text_field(package, NAME, METADATA_COMMAND, &text)?;
        names.insert(id.to_owned(), name.to_owned());

        for target in array_field(package, TARGETS, METADATA_COMMAND, &text)? {
            if !bool_field(target, DOC_FIELD, METADATA_COMMAND, &text)? {
                continue;
            }
            documentable.insert(target_of(name, target, METADATA_COMMAND, &text)?);
        }
    }

    if names.is_empty() {
        return Err(DocLinksError::NoWorkspaceMembers {
            dir: dir.to_path_buf(),
        });
    }

    let target_directory = text_field(&metadata, TARGET_DIRECTORY, METADATA_COMMAND, &text)?;
    Ok(Workspace {
        names,
        documentable,
        doc_dir: Path::new(target_directory).join(DOC_SUBDIR),
    })
}

/// Read the JSON Lines `cargo doc` wrote: the links rustdoc could not resolve,
/// and the targets the build documented.
///
/// Both come out of one pass, because they are two readings of the same
/// transcript and a second pass could disagree with the first.
fn read_build(stdout: &str, workspace: &Workspace) -> Result<DocScan, DocLinksError> {
    let mut broken = Vec::new();
    let mut documented = BTreeSet::new();

    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let record: Value = serde_json::from_str(line).map_err(|source| DocLinksError::Json {
            command: DOC_COMMAND,
            line: elide(line),
            source,
        })?;

        match text_field(&record, REASON, DOC_COMMAND, line)? {
            COMPILER_MESSAGE => {
                if let Some(link) = unresolved_link(&record, workspace, line)? {
                    broken.push(link);
                }
            }
            COMPILER_ARTIFACT => {
                if let Some(target) = documented_target(&record, workspace, line)? {
                    documented.insert(target);
                }
            }
            _ => {}
        }
    }

    Ok(DocScan { broken, documented })
}

/// The unresolved link one diagnostic reports, or `None` when the diagnostic is
/// some other lint.
fn unresolved_link(
    record: &Value,
    workspace: &Workspace,
    line: &str,
) -> Result<Option<BrokenLink>, DocLinksError> {
    let message = field(record, MESSAGE, DOC_COMMAND, line)?;
    let Some(code) = message
        .get(CODE)
        .and_then(|code| code.get(CODE))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    if code != BROKEN_INTRA_DOC_LINKS {
        return Ok(None);
    }
    broken_link(record, message, &workspace.names, line).map(Some)
}

/// The target one artifact record documented, or `None` when the record reports
/// a unit that was compiled rather than documented, or a unit of a package
/// outside the workspace.
///
/// The test is where the files landed. `cargo doc --no-deps` still *compiles*
/// every member another member depends on, and each of those compilations
/// reports an artifact too — with `.rmeta` files under `target/debug/deps`
/// rather than pages under `target/doc`. The same package can therefore report
/// both kinds of artifact for the same target in one pass. Counting artifacts
/// without this test would count those compilations as documentation, so the
/// parity check would pass while units went unread.
fn documented_target(
    record: &Value,
    workspace: &Workspace,
    line: &str,
) -> Result<Option<DocTarget>, DocLinksError> {
    let package_id = text_field(record, PACKAGE_ID, DOC_COMMAND, line)?;
    let Some(package) = workspace.names.get(package_id) else {
        return Ok(None);
    };

    let mut documented = false;
    for filename in array_field(record, FILENAMES, DOC_COMMAND, line)? {
        let path = Path::new(as_text(filename, FILENAMES, DOC_COMMAND, line)?);
        if path.starts_with(&workspace.doc_dir) {
            documented = true;
            break;
        }
    }
    if !documented {
        return Ok(None);
    }

    let target = field(record, TARGET, DOC_COMMAND, line)?;
    target_of(package, target, DOC_COMMAND, line).map(Some)
}

/// One target of `package`, read out of the `target` object cargo writes into
/// both its metadata and its build records.
///
/// The two spellings are the same object, so they are read by the same
/// function. A second reader would be a second model of one format, and the two
/// would disagree about which targets the build reached — silently, since a
/// disagreement there reads as an unread target rather than as an error.
fn target_of(
    package: &str,
    target: &Value,
    command: &'static str,
    source: &str,
) -> Result<DocTarget, DocLinksError> {
    Ok(DocTarget {
        package: package.to_owned(),
        name: text_field(target, NAME, command, source)?.to_owned(),
        kind: first_kind(target, command, source)?.to_owned(),
    })
}

/// The first kind of a target.
///
/// Cargo writes an array, and keys everything else on its first entry, so the
/// guard does too. An empty array is a refusal: a target of no kind cannot be
/// asked for by name, and a guard that dropped it would report clean for a unit
/// it never read.
fn first_kind<'a>(
    target: &'a Value,
    command: &'static str,
    source: &str,
) -> Result<&'a str, DocLinksError> {
    let kinds = array_field(target, KIND, command, source)?;
    let kind = kinds.first().ok_or_else(|| DocLinksError::MissingField {
        command,
        field: KIND,
        record: elide(source),
    })?;
    as_text(kind, KIND, command, source)
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
    let span = primary_span(message, line)?;

    Ok(BrokenLink {
        target: target_of(package, target, DOC_COMMAND, line)?,
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

/// One boolean field of a record.
fn bool_field(
    record: &Value,
    key: &'static str,
    command: &'static str,
    source: &str,
) -> Result<bool, DocLinksError> {
    field(record, key, command, source)?
        .as_bool()
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

#[cfg(test)]
mod tests {
    //! What each refusal says when a record is not the shape the guard reads.
    //!
    //! The readers below are private, and driving a whole cargo invocation to
    //! reach them would be slower and less precise than handing them the one
    //! record under test. Every assertion is on the rendered message, because
    //! the words are the product: this module's value is the precision of its
    //! refusals, and "no `spans`" printed over a record that has `spans` sends
    //! the reader after a defect that is not there.
    //!
    //! Parallel-safe and hermetic by construction: every fixture is a
    //! `serde_json::Value` built in memory, so nothing here names a file, a
    //! port, an environment variable, or any other shared resource.

    use serde_json::json;

    use super::{
        DOC_COMMAND, DocLinksError, FILE_NAME, KIND, LINE_START, METADATA_COMMAND, PACKAGE_ID,
        SPANS, WORKSPACE_MEMBERS, array_field, as_text, bool_field, field, first_kind,
        number_field, primary_span, text_field,
    };

    /// The refusal a reader handed back, in the words a human reads.
    ///
    /// A reader that returns a value read something it should have refused, and
    /// that is a failure of the test rather than of the message.
    fn refusal<T>(result: Result<T, DocLinksError>) -> String {
        match result {
            Ok(_) => panic!("the reader accepted a record it cannot read"),
            Err(error) => error.to_string(),
        }
    }

    /// A field that is genuinely absent still reads as absent. The refusals for
    /// a wrong type and for an empty array are additions, not a rename.
    #[test]
    fn an_absent_field_is_reported_as_absent() {
        let record = json!({ "reason": "compiler-artifact" });
        let source = record.to_string();

        assert_eq!(
            refusal(field(&record, PACKAGE_ID, DOC_COMMAND, &source)),
            format!("a `cargo doc` record has no `{PACKAGE_ID}`:\n  {source}")
        );
    }

    /// A diagnostic that carries no `spans` key at all is the case the absence
    /// refusal describes, and it keeps that refusal.
    #[test]
    fn an_absent_spans_array_is_reported_as_absent() {
        let message = json!({ "message": "unresolved link to `run`" });
        let source = message.to_string();

        assert_eq!(
            refusal(primary_span(&message, &source)),
            format!("a `cargo doc` record has no `{SPANS}`:\n  {source}")
        );
    }

    /// A field the guard reads as a string, holding an array.
    #[test]
    fn a_string_field_holding_an_array_names_both_types() {
        let record = json!({ "file_name": ["src/lib.rs"] });
        let source = record.to_string();

        assert_eq!(
            refusal(text_field(&record, FILE_NAME, DOC_COMMAND, &source)),
            format!(
                "a `cargo doc` record has a `{FILE_NAME}` that is an array, not a string:\n  {source}"
            )
        );
    }

    /// An array entry the guard reads as a string, holding a number. The entry
    /// is named by the array it came out of, which is the only name it has.
    #[test]
    fn an_array_entry_that_is_not_a_string_names_both_types() {
        let source = json!({ "workspace_members": [7] }).to_string();

        assert_eq!(
            refusal(as_text(&json!(7), WORKSPACE_MEMBERS, METADATA_COMMAND, &source)),
            format!(
                "a `cargo metadata` record has a `{WORKSPACE_MEMBERS}` that is a number, not a string:\n  {source}"
            )
        );
    }

    /// A field the guard reads as an array, holding an object.
    #[test]
    fn an_array_field_holding_an_object_names_both_types() {
        let record = json!({ "kind": { "bin": true } });
        let source = record.to_string();

        assert_eq!(
            refusal(array_field(&record, KIND, DOC_COMMAND, &source)),
            format!(
                "a `cargo doc` record has a `{KIND}` that is an object, not an array:\n  {source}"
            )
        );
    }

    /// A field the guard reads as a boolean, holding a string. `"true"` is the
    /// spelling most likely to arrive, and it is not a boolean.
    #[test]
    fn a_boolean_field_holding_a_string_names_both_types() {
        let record = json!({ "doc": "true" });
        let source = record.to_string();

        assert_eq!(
            refusal(bool_field(&record, super::DOC_FIELD, METADATA_COMMAND, &source)),
            format!(
                "a `cargo metadata` record has a `doc` that is a string, not a boolean:\n  {source}"
            )
        );
    }

    /// A field the guard reads as an unsigned integer, holding a string.
    #[test]
    fn a_number_field_holding_a_string_names_both_types() {
        let record = json!({ "line_start": "12" });
        let source = record.to_string();

        assert_eq!(
            refusal(number_field(&record, LINE_START, DOC_COMMAND, &source)),
            format!(
                "a `cargo doc` record has a `{LINE_START}` that is a string, not an unsigned integer:\n  {source}"
            )
        );
    }

    /// A field the guard reads as an unsigned integer, holding a number that is
    /// not one. What the guard wanted comes from the reader, not from the value
    /// it got, so a negative number is still refused for not being unsigned.
    #[test]
    fn a_number_field_holding_a_negative_number_names_what_the_reader_wanted() {
        let record = json!({ "line_start": -12 });
        let source = record.to_string();

        assert_eq!(
            refusal(number_field(&record, LINE_START, DOC_COMMAND, &source)),
            format!(
                "a `cargo doc` record has a `{LINE_START}` that is a number, not an unsigned integer:\n  {source}"
            )
        );
    }

    /// A diagnostic whose `spans` array is present and holds nothing. The
    /// absence refusal would send the reader looking for a key that is there.
    #[test]
    fn an_empty_spans_array_is_reported_as_empty() {
        let message = json!({ "spans": [] });
        let source = message.to_string();

        assert_eq!(
            refusal(primary_span(&message, &source)),
            format!("a `cargo doc` record has an empty `{SPANS}` array:\n  {source}")
        );
    }

    /// A target whose `kind` array is present and holds nothing.
    #[test]
    fn an_empty_kind_array_is_reported_as_empty() {
        let target = json!({ "name": "aa", "kind": [] });
        let source = target.to_string();

        assert_eq!(
            refusal(first_kind(&target, METADATA_COMMAND, &source)),
            format!("a `cargo metadata` record has an empty `{KIND}` array:\n  {source}")
        );
    }

    /// A `kind` array that holds something that is not a target kind.
    #[test]
    fn a_kind_entry_that_is_not_a_string_names_both_types() {
        let target = json!({ "name": "aa", "kind": [7] });
        let source = target.to_string();

        assert_eq!(
            refusal(first_kind(&target, METADATA_COMMAND, &source)),
            format!(
                "a `cargo metadata` record has a `{KIND}` that is a number, not a string:\n  {source}"
            )
        );
    }
}
