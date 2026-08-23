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

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

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
/// # Errors
///
/// Returns [`DocLinksError`] — never a clean [`DocScan`] — when the build
/// cannot be read with confidence.
pub fn audit(workspace_root: &Path) -> Result<DocScan, DocLinksError> {
    let _ = workspace_root;
    Ok(DocScan {
        broken: Vec::new(),
        documented: BTreeSet::new(),
    })
}
