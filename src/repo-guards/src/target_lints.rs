//! Guard: every *target* of a crate must declare a position on the lints that
//! crate raises.
//!
//! [`workspace_lints`](crate::workspace_lints) proves each member crate opts
//! into the repo-wide lint set. This module is the same disease one level down,
//! at the target instead of the crate.
//!
//! Cargo refuses to merge a member's own `[lints]` table with
//! `lints.workspace = true` (`error: cannot override workspace.lints in
//! lints`). So a crate that wants lints *stricter* than the workspace set has
//! exactly one place to put them: crate-root inner attributes in its source,
//! e.g.
//!
//! ```ignore
//! #![deny(unsafe_code)]
//! #![warn(clippy::pedantic)]
//! ```
//!
//! And that is where the hole opens. A manifest `[lints]` table applies to
//! **every target of the package**. A crate-root attribute applies to **one
//! target** — the single file that is that target's root. Moving lints out of
//! the manifest to satisfy the inheritance guard therefore hands the library
//! and binary a stricter lint set while the integration tests, benches, and
//! examples of the same crate quietly keep the workspace default. Nothing warns.
//! The exemption is spelled as an *absence*, which is why it is invisible and
//! why it spreads.
//!
//! # The rule
//!
//! For each workspace member:
//!
//! 1. The crate's **baseline** is the union of the lint paths *raised* — `deny`,
//!    `forbid`, or `warn` — by inner attributes at the roots of its **library
//!    and binary** targets. Those are the targets that define what the crate
//!    holds itself to.
//! 2. Every target root of that crate — library, binary, test, bench, example —
//!    must **mention** every baseline lint in some inner lint attribute:
//!    `deny`, `forbid`, `warn`, `allow`, or `expect`.
//! 3. **Silence is the only violation.**
//!
//! # Why "mention", not "match the level"
//!
//! This guard deliberately does **not** compare levels. A test root that
//! `allow`s a baseline lint is *compliant*:
//!
//! ```ignore
//! #![allow(
//!     clippy::unwrap_used,
//!     reason = "a test that cannot unwrap is a test written around its harness"
//! )]
//! ```
//!
//! That is a deliberate, visible, reviewable decision sitting in the file it
//! applies to. A root that says *nothing* is not a decision at all — it is the
//! absence this guard exists to convert into a declaration. Demanding equal
//! levels would be a different guard with a different, worse trade: integration
//! tests genuinely do need to unwrap, and a rule they cannot satisfy gets
//! satisfied by deleting the rule.
//!
//! That is also why there is **no central exemption file**. The local
//! `#![allow(lint, reason = "...")]` *is* the opt-out mechanism, and the
//! workspace's own `clippy::allow_attributes_without_reason` already forces the
//! reason to be written next to it. An allowlist elsewhere would move the
//! decision away from the code it exempts, which is how exemptions become
//! permanent.
//!
//! # Parse, never text-match
//!
//! "An inner attribute whose path is a lint level and whose arguments are lint
//! paths" names a syntactic category, so it is answered with [`syn`], not a
//! regex. Every spelling therefore reduces to the same answer: one lint per
//! attribute or several, `clippy::pedantic` or bare `unsafe_code`, with or
//! without a trailing `reason = "..."` (a name-value pair, never a lint name).
//! A regex would answer only for the spellings someone happened to think of,
//! and a missed spelling reports *clean* — indistinguishable from a guard doing
//! real work.
//!
//! One consequence worth stating: `#![cfg_attr(not(test), warn(lint))]` is an
//! attribute whose path is `cfg_attr`, not a lint level, so it neither raises
//! nor mentions anything. That is correct rather than incidental — a
//! conditionally-applied lint is not a position the crate holds in every
//! configuration.
//!
//! Everything that could shrink the audited set is a hard error rather than a
//! clean verdict; see [`TargetLintsError`].

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::{AttrStyle, Meta, Token};
use thiserror::Error;
use toml::Value;

use crate::workspace_lints::{self, WorkspaceLintsError, CARGO_TOML};

/// Attribute paths that *raise* a lint, and so contribute to a crate's baseline.
const RAISING_LEVELS: [&str; 3] = ["deny", "forbid", "warn"];

/// Attribute paths that *mention* a lint. A superset of [`RAISING_LEVELS`]:
/// `allow` and `expect` are positions too, just not raised ones.
const MENTIONING_LEVELS: [&str; 5] = ["allow", "deny", "expect", "forbid", "warn"];

/// `[package]` keys that turn cargo's target auto-discovery on or off.
///
/// This guard models the default discovery rules only. A manifest that changes
/// them is refused rather than guessed at; see
/// [`AutoDiscoveryOverride`](TargetLintsError::AutoDiscoveryOverride).
const AUTO_DISCOVERY_KEYS: [&str; 4] = ["autobenches", "autobins", "autoexamples", "autotests"];

/// Rust source extension, used when auto-discovering target roots.
const RS: &str = "rs";

/// The conventional entry point of a directory-shaped target
/// (`tests/foo/main.rs`).
const MAIN_RS: &str = "main.rs";

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot enumerate a crate's
/// targets must say so loudly, because "no target is missing a lint" and "I
/// found no targets" are the same sentence to a CI log and only one of them is
/// good news.
#[derive(Debug, Error)]
pub enum TargetLintsError {
    /// The workspace members could not be enumerated.
    #[error("cannot enumerate the workspace members: {0}")]
    Members(#[from] WorkspaceLintsError),

    /// A target root could not be read from disk.
    #[error("cannot read the target root {}: {source}", path.display())]
    ReadRoot {
        /// The root file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A target root was read but is not valid Rust.
    #[error("cannot parse {} as Rust: {source}", path.display())]
    ParseRoot {
        /// The root file that failed to parse.
        path: PathBuf,
        /// The underlying parse failure.
        source: syn::Error,
    },

    /// A directory that should hold target roots could not be listed.
    #[error("cannot list {} while discovering target roots: {source}", dir.display())]
    ReadTargetDir {
        /// The directory that could not be listed.
        dir: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A manifest declares a target `path` that is not on disk.
    #[error(
        "{} declares a {kind} target at `{declared}`, which does not exist; \
         a target this guard cannot open is a target it cannot vouch for",
        manifest.display()
    )]
    MissingDeclaredTarget {
        /// The manifest carrying the declaration.
        manifest: PathBuf,
        /// Which kind of target declared it: `lib`, `bin`, `test`, …
        kind: &'static str,
        /// The `path` value, exactly as written in the manifest.
        declared: String,
    },

    /// A manifest overrides cargo's target auto-discovery.
    #[error(
        "{} sets `{key}`; this guard models cargo's default target discovery only, \
         so it would enumerate the wrong roots and report clean for the wrong reason",
        manifest.display()
    )]
    AutoDiscoveryOverride {
        /// The manifest carrying the override.
        manifest: PathBuf,
        /// The offending key: `autotests`, `autobins`, …
        key: &'static str,
    },

    /// A member crate resolved to no target roots at all.
    #[error(
        "workspace member {} resolves to no target roots; refusing to report a crate clean \
         when nothing in it was examined",
        dir.display()
    )]
    NoTargetRoots {
        /// The member directory.
        dir: PathBuf,
    },
}

/// One target root that is silent about at least one of its crate's baseline
/// lints.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The root file, relative to the repo root.
    path: PathBuf,
    /// Which kind of target this root is: `library`, `binary`, `test`, …
    kind: &'static str,
    /// Baseline lints this root mentions nowhere, sorted.
    missing: Vec<String>,
}

impl Offender {
    /// The offending target root, relative to the repo root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Which kind of target this root is: `library`, `binary`, `test`, `bench`,
    /// or `example`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The baseline lints this root mentions nowhere, sorted.
    #[must_use]
    pub fn missing(&self) -> &[String] {
        &self.missing
    }
}

/// The verdict of one audit: how much was examined, and which target roots are
/// silent about their crate's baseline lints.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Number of member crates whose targets were resolved.
    crates_examined: usize,
    /// Number of target roots actually opened and parsed.
    roots_examined: usize,
    /// Silent roots, sorted by path.
    offenders: Vec<Offender>,
}

impl Report {
    /// True when every examined target root declares a position on every
    /// baseline lint of its crate.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The target roots that are silent about a baseline lint, sorted by path.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// How many member crates the audit resolved targets for.
    #[must_use]
    pub fn crates_examined(&self) -> usize {
        self.crates_examined
    }

    /// How many target roots the audit opened and parsed.
    ///
    /// A caller should assert this is non-zero: a guard that scans nothing
    /// reports clean for the wrong reason.
    #[must_use]
    pub fn roots_examined(&self) -> usize {
        self.roots_examined
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Checked {} target roots across {} workspace members; every one declares a position on its crate's lints.",
                self.roots_examined, self.crates_examined
            );
        }

        writeln!(
            f,
            "{} of {} target roots are silent about a lint their crate raises.",
            self.offenders.len(),
            self.roots_examined
        )?;
        writeln!(
            f,
            "A crate-root lint attribute applies to ONE target; the other targets of the same crate keep the workspace default unless they say otherwise."
        )?;

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} ({} target) never mentions: {}",
                offender.path.display(),
                offender.kind,
                offender.missing.join(", ")
            )?;
            writeln!(
                f,
                "Add an inner attribute at the top of that file taking a position on each — raise it:"
            )?;
            writeln!(f)?;
            for lint in &offender.missing {
                writeln!(f, "    #![warn({lint})]")?;
            }
            writeln!(f)?;
            writeln!(f, "or opt out of it on purpose, in writing:")?;
            writeln!(f)?;
            for lint in &offender.missing {
                writeln!(f, "    #![allow({lint}, reason = \"...\")]")?;
            }
        }

        Ok(())
    }
}

/// Which cargo target a root file belongs to.
///
/// The distinction that matters is [`raises_baseline`](TargetKind::raises_baseline):
/// libraries and binaries *define* what a crate holds itself to; tests, benches,
/// and examples only have to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Lib,
    Bin,
    Test,
    Bench,
    Example,
}

impl TargetKind {
    /// The manifest key that declares this kind of target explicitly.
    const fn manifest_key(self) -> &'static str {
        match self {
            Self::Lib => "lib",
            Self::Bin => "bin",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Example => "example",
        }
    }

    /// The human-readable name used in [`Report`]'s message.
    const fn label(self) -> &'static str {
        match self {
            Self::Lib => "library",
            Self::Bin => "binary",
            Self::Test => "test",
            Self::Bench => "bench",
            Self::Example => "example",
        }
    }

    /// True for the targets whose raised lints form the crate's baseline.
    const fn raises_baseline(self) -> bool {
        matches!(self, Self::Lib | Self::Bin)
    }
}

/// One resolved target root: the file, and what kind of target it heads.
#[derive(Debug, Clone)]
struct TargetRoot {
    path: PathBuf,
    kind: TargetKind,
}

/// The lint positions taken at one target root.
#[derive(Debug, Default)]
struct RootLints {
    /// Lints raised here (`deny`/`forbid`/`warn`).
    raised: BTreeSet<String>,
    /// Lints mentioned here at any level, raised ones included.
    mentioned: BTreeSet<String>,
}

/// Audit every target of every member of the workspace rooted at `repo_root`.
///
/// Members come from [`workspace_lints::members`], so both guards walk exactly
/// the same set. For each member the targets are resolved from its manifest
/// plus cargo's default discovery rules, every root is parsed, and any root that
/// is silent about a lint its crate's library or binary raises is reported.
///
/// # Errors
///
/// Returns [`TargetLintsError`] — never a clean [`Report`] — when the targets
/// cannot be enumerated or read with confidence:
///
/// - the workspace members cannot be enumerated
///   ([`Members`](TargetLintsError::Members));
/// - a member manifest is unreadable or not valid TOML (also
///   [`Members`](TargetLintsError::Members), carrying the underlying
///   [`WorkspaceLintsError`]);
/// - a manifest declares a target `path` that is not on disk
///   ([`MissingDeclaredTarget`](TargetLintsError::MissingDeclaredTarget));
/// - a manifest sets `autotests`, `autobins`, `autobenches`, or `autoexamples`
///   ([`AutoDiscoveryOverride`](TargetLintsError::AutoDiscoveryOverride));
/// - a directory holding target roots cannot be listed
///   ([`ReadTargetDir`](TargetLintsError::ReadTargetDir));
/// - a member resolves to no target roots
///   ([`NoTargetRoots`](TargetLintsError::NoTargetRoots));
/// - a root file is unreadable ([`ReadRoot`](TargetLintsError::ReadRoot)) or
///   does not parse as Rust ([`ParseRoot`](TargetLintsError::ParseRoot)).
pub fn audit(repo_root: &Path) -> Result<Report, TargetLintsError> {
    let member_dirs = workspace_lints::members(repo_root)?;

    let mut roots_examined = 0;
    let mut offenders = Vec::new();

    for dir in &member_dirs {
        let manifest_path = dir.join(CARGO_TOML);
        let manifest = workspace_lints::parse_manifest(&manifest_path)?;
        let roots = target_roots(dir, &manifest, &manifest_path)?;

        let mut scanned = Vec::with_capacity(roots.len());
        for root in roots {
            let lints = root_lints(&root.path)?;
            scanned.push((root, lints));
        }
        roots_examined += scanned.len();

        let baseline: BTreeSet<&String> = scanned
            .iter()
            .filter(|(root, _)| root.kind.raises_baseline())
            .flat_map(|(_, lints)| lints.raised.iter())
            .collect();
        if baseline.is_empty() {
            continue;
        }

        for (root, lints) in &scanned {
            let missing: Vec<String> = baseline
                .iter()
                .filter(|lint| !lints.mentioned.contains(**lint))
                .map(|lint| (*lint).clone())
                .collect();
            if missing.is_empty() {
                continue;
            }
            offenders.push(Offender {
                path: root
                    .path
                    .strip_prefix(repo_root)
                    .unwrap_or(&root.path)
                    .to_path_buf(),
                kind: root.kind.label(),
                missing,
            });
        }
    }

    offenders.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Report {
        crates_examined: member_dirs.len(),
        roots_examined,
        offenders,
    })
}

/// Resolve every target root of one member crate.
///
/// Mirrors cargo: explicit `path` declarations win, and the conventional
/// locations are discovered on top of them (`src/lib.rs`, `src/main.rs`,
/// `src/bin/*.rs`, `tests/*.rs`, `benches/*.rs`, `examples/*.rs`, plus the
/// directory form `<dir>/<name>/main.rs`). Discovery is deliberately depth-1:
/// `tests/common/mod.rs` is a module a test root *includes*, not a target root,
/// and treating it as one would invent a target cargo never builds.
fn target_roots(
    dir: &Path,
    manifest: &Value,
    manifest_path: &Path,
) -> Result<Vec<TargetRoot>, TargetLintsError> {
    for key in AUTO_DISCOVERY_KEYS {
        if manifest
            .get("package")
            .and_then(|package| package.get(key))
            .is_some()
        {
            return Err(TargetLintsError::AutoDiscoveryOverride {
                manifest: manifest_path.to_path_buf(),
                key,
            });
        }
    }

    let mut roots = Vec::new();

    // Library: an explicit `[lib] path` wins, otherwise the conventional root.
    match declared_path(manifest.get(TargetKind::Lib.manifest_key())) {
        Some(declared) => roots.push(declared_root(
            dir,
            declared,
            TargetKind::Lib,
            manifest_path,
        )?),
        None => push_if_file(&mut roots, dir.join("src").join("lib.rs"), TargetKind::Lib),
    }

    // Binaries: every explicit `[[bin]] path`, plus the conventional roots.
    for declared in declared_paths(manifest.get(TargetKind::Bin.manifest_key())) {
        roots.push(declared_root(
            dir,
            declared,
            TargetKind::Bin,
            manifest_path,
        )?);
    }
    push_if_file(&mut roots, dir.join("src").join("main.rs"), TargetKind::Bin);
    discover(&mut roots, &dir.join("src").join("bin"), TargetKind::Bin)?;

    // Tests, benches, and examples: same shape, different directory.
    for (kind, subdir) in [
        (TargetKind::Test, "tests"),
        (TargetKind::Bench, "benches"),
        (TargetKind::Example, "examples"),
    ] {
        for declared in declared_paths(manifest.get(kind.manifest_key())) {
            roots.push(declared_root(dir, declared, kind, manifest_path)?);
        }
        discover(&mut roots, &dir.join(subdir), kind)?;
    }

    // An explicit declaration and the conventional location often name the same
    // file (`[[bin]] path = "src/main.rs"` is the norm in this repo); auditing
    // it twice would double-count and double-report.
    roots.sort_by(|a, b| a.path.cmp(&b.path));
    roots.dedup_by(|a, b| a.path == b.path);

    if roots.is_empty() {
        return Err(TargetLintsError::NoTargetRoots {
            dir: dir.to_path_buf(),
        });
    }
    Ok(roots)
}

/// The `path` of a single-target table such as `[lib]`.
fn declared_path(table: Option<&Value>) -> Option<&str> {
    table
        .and_then(|target| target.get("path"))
        .and_then(Value::as_str)
}

/// The `path` of every entry of an array-of-tables such as `[[bin]]`.
fn declared_paths(array: Option<&Value>) -> Vec<&str> {
    array
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| declared_path(Some(entry)))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve one manifest-declared target path, refusing if it is not on disk.
fn declared_root(
    dir: &Path,
    declared: &str,
    kind: TargetKind,
    manifest_path: &Path,
) -> Result<TargetRoot, TargetLintsError> {
    let path = dir.join(declared);
    if !path.is_file() {
        return Err(TargetLintsError::MissingDeclaredTarget {
            manifest: manifest_path.to_path_buf(),
            kind: kind.manifest_key(),
            declared: declared.to_owned(),
        });
    }
    Ok(TargetRoot { path, kind })
}

/// Append `path` as a root of `kind` when it exists.
fn push_if_file(roots: &mut Vec<TargetRoot>, path: PathBuf, kind: TargetKind) {
    if path.is_file() {
        roots.push(TargetRoot { path, kind });
    }
}

/// Auto-discover the roots cargo would find in `dir`: every depth-1 `*.rs`
/// file, plus `<subdir>/main.rs` for each immediate subdirectory.
///
/// A missing directory is not an error — most crates have no `benches/`. A
/// directory that exists but cannot be listed *is* one: silently skipping it
/// would drop targets from the audit and report clean for the wrong reason.
fn discover(
    roots: &mut Vec<TargetRoot>,
    dir: &Path,
    kind: TargetKind,
) -> Result<(), TargetLintsError> {
    if !dir.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(dir).map_err(|source| TargetLintsError::ReadTargetDir {
        dir: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| TargetLintsError::ReadTargetDir {
                dir: dir.to_path_buf(),
                source,
            })?
            .path();

        if path.is_dir() {
            push_if_file(roots, path.join(MAIN_RS), kind);
        } else if path.extension().is_some_and(|ext| ext == RS) {
            roots.push(TargetRoot { path, kind });
        }
    }
    Ok(())
}

/// Read one target root and collect the lint positions its inner attributes
/// take.
///
/// Only [`AttrStyle::Inner`] attributes count: `#![warn(...)]` configures the
/// whole target, while an outer `#[warn(...)]` on the first item configures only
/// that item and would be a false positive if counted.
///
/// `reason = "..."` is a [`Meta::NameValue`], not a [`Meta::Path`], so it is
/// skipped rather than collected as a lint named `reason`.
fn root_lints(path: &Path) -> Result<RootLints, TargetLintsError> {
    let text = fs::read_to_string(path).map_err(|source| TargetLintsError::ReadRoot {
        path: path.to_path_buf(),
        source,
    })?;
    let file = syn::parse_file(&text).map_err(|source| TargetLintsError::ParseRoot {
        path: path.to_path_buf(),
        source,
    })?;

    let mut lints = RootLints::default();
    for attr in &file.attrs {
        if !matches!(attr.style, AttrStyle::Inner(_)) {
            continue;
        }
        let Meta::List(list) = &attr.meta else {
            continue;
        };
        let Some(level) = sole_segment(&list.path) else {
            continue;
        };
        if !MENTIONING_LEVELS.contains(&level.as_str()) {
            continue;
        }

        let args = list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|source| TargetLintsError::ParseRoot {
                path: path.to_path_buf(),
                source,
            })?;
        for arg in args {
            let Meta::Path(lint) = arg else {
                continue;
            };
            let name = render_path(&lint);
            if RAISING_LEVELS.contains(&level.as_str()) {
                lints.raised.insert(name.clone());
            }
            lints.mentioned.insert(name);
        }
    }
    Ok(lints)
}

/// The identifier of a single-segment path, or `None` for anything longer.
///
/// Lint *levels* are always one segment (`warn`), which is what distinguishes
/// them from wrappers like `cfg_attr` only by name — and from lint *paths* like
/// `clippy::pedantic` by shape.
fn sole_segment(path: &syn::Path) -> Option<String> {
    match path.segments.len() {
        1 => path.segments.first().map(|seg| seg.ident.to_string()),
        _ => None,
    }
}

/// Render a lint path the way a human writes it: segments joined with `::`, so
/// `clippy::pedantic` and bare `unsafe_code` both come back verbatim.
fn render_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}
