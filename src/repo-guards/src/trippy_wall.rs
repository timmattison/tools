//! Guard: no file of `krt` except `trace.rs` names a type from a trippy crate.
//!
//! `krt` records a network path with the `trippy-core` and `trippy-privilege`
//! crates. The documentation of `trippy-core` says that its public API is not
//! stable, and that it is highly likely to change. Two rules answer that risk.
//! The manifest pins the exact version, and one module — `trace.rs` — is the
//! only place that names a trippy type. Every other file speaks to the tracer
//! through types that `krt` owns. An upgrade of the trippy crates then breaks
//! one file, and the compiler shows the whole break in that one file.
//!
//! The first rule needs no guard, because a pinned version is visible in the
//! manifest. The second rule needs one. A trippy path in the wrong file looks
//! like an ordinary line of code, and nothing fails when a person writes it. So
//! it spreads: the module that needs one field of a trippy struct takes the
//! struct, and the wall is gone before a reviewer reads the diff.
//!
//! # The whole package, not one directory of it
//!
//! `trippy-core` is an ordinary `[dependencies]` entry of `krt`, so *every*
//! target of the package can name a trippy type: the binary, and both
//! integration tests. A wall around `src/` alone would keep a smaller promise
//! than the one this module makes, and would keep it silently — an integration
//! test that took a trippy type would compile, pass, and leave the guard
//! reporting clean. The `KRT_DIRECTORIES` table therefore names every directory
//! of the package, in one place, and a companion test asks `cargo metadata`
//! whether that set still covers the roots cargo builds.
//!
//! # Parse, never text-match
//!
//! "A path whose first segment names a trippy crate" is a syntactic category,
//! so [`syn`] answers it and a regex does not. A regex answers only for the
//! spellings that somebody thought of, and a spelling it never learned reports
//! *clean*, which reads the same as a guard that does real work. The four
//! checks below are the four shapes that one fact arrives in:
//!
//! 1. **A path.** Any [`syn::Path`] whose first segment identifier starts with
//!    `trippy`. This covers a type in a signature, a fully-qualified call, an
//!    associated constant, a turbofish, and an attribute path. A leading `::`
//!    is not a segment, so `::trippy_core::Builder` starts at `trippy_core` and
//!    fires.
//! 2. **A `use` tree.** A [`syn::ItemUse`] holds a [`syn::UseTree`], not a
//!    [`syn::Path`], so the path check never sees it. `use trippy_core as tc;`
//!    is the shape that makes this check necessary, because every later mention
//!    in that file says `tc`.
//! 3. **An `extern crate`.** A [`syn::ItemExternCrate`] holds a bare
//!    identifier, which is again not a path.
//! 4. **An identifier in a token stream.** `syn` hands a macro body over as
//!    unparsed tokens, so a path check never sees
//!    `println!("{:?}", trippy_core::MAX_TTL)`. The walk goes into every
//!    [`proc_macro2::Group`] and reads an [`proc_macro2::Ident`] only. It never
//!    reads a [`proc_macro2::Literal`], so the string `"trippy"` in a macro body
//!    does not fire.
//!
//! Check 4 can over-match: a local binding named `trippy_thing` inside a macro
//! body is flagged, and it names no trippy type. That trade is deliberate. An
//! over-match fails loudly, and a person corrects it in an hour. An under-match
//! stays green for years.
//!
//! # The allowed module keeps what it holds
//!
//! The wall lets `trace.rs` name a trippy type, and that permission has one
//! limit. `pub(crate) use trippy_core::Port as LeakedPort;` inside `trace.rs`
//! puts that type into every other module of `krt` under a `crate::trace::`
//! name, and every one of those modules then breaks on the upgrade that the
//! wall promises breaks one file. The four checks above never see it, because
//! they never run on the allowed module.
//!
//! So the allowed module gets a check of its own, and it is a narrow one: a
//! `use` inside it whose tree is rooted at a trippy crate, and whose visibility
//! reaches past the module that writes it, is an offender. Nothing else about
//! the allowed module changes. A private `use trippy_core::{...};` is how the
//! tracer names a trippy type at all, and it stays the normal case.
//!
//! The two faults carry two remedies, so [`Report`] says which one a file hit.
//! Code outside the tracer that names a trippy type belongs in the tracer. A
//! re-export is already in the right file, and telling its author to move it
//! sends them nowhere.
//!
//! # Refuse rather than shrink
//!
//! Everything that stops the guard from reading a source is an error, never a
//! clean verdict. A file that does not parse is a file whose paths the guard
//! cannot see, which is not the same as a file that holds none. A directory
//! with no Rust source in it is a guard pointed at the wrong place, and
//! "I examined nothing" reads exactly like "everything is clean". See
//! [`TrippyWallError`].

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::Visit;
use thiserror::Error;

/// The one module of `krt` which names a trippy type.
const TRACE_MODULE: &str = "trace.rs";

/// One directory of `krt` that the wall audits, and the module inside it that
/// the wall lets through.
#[derive(Debug, Clone, Copy)]
struct AuditedDirectory {
    /// The directory, relative to the repository root.
    path: &'static str,
    /// The module inside `path` that names a trippy type, or `None` when no
    /// file in `path` is allowed to name one.
    allowed_module: Option<&'static str>,
}

/// Every directory of `krt` the wall audits, stated once.
///
/// A directory that is absent from this table is a directory nobody reads, and
/// a file nobody reads cannot be reported as naming a trippy type. So a new
/// target of `krt` — a bench, an example, a second directory of tests — belongs
/// here on the day it is written.
///
/// Nothing in this table is derived from `cargo metadata`, and that is
/// deliberate: a guard that spawns cargo on every run is a guard that gets
/// deleted from the test suite. The companion test
/// `every_target_root_of_krt_is_a_file_the_guard_read` pays that cost once, and
/// asks cargo whether this table still covers the roots it builds. A target
/// kind nobody taught the guard about then arrives as a set difference rather
/// than as a clean report.
const KRT_DIRECTORIES: [AuditedDirectory; 2] = [
    AuditedDirectory {
        path: "src/krt/src",
        allowed_module: Some(TRACE_MODULE),
    },
    // The tracer is one file, and it lives in the source directory. An
    // integration test speaks to `krt` by running the binary, so nothing here
    // needs a trippy type — and a test file that happened to be named
    // `trace.rs` must not inherit the exemption by coincidence of naming.
    AuditedDirectory {
        path: "src/krt/tests",
        allowed_module: None,
    },
];

/// The first characters of the name of every trippy crate.
const TRIPPY: &str = "trippy";

/// The extension of a Rust source file.
const RS: &str = "rs";

/// The separator between the segments of a rendered path.
const SEPARATOR: &str = "::";

/// Everything that stops the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot read a source must say so
/// loudly, because "no module names a trippy type" and "I read no modules" are
/// the same sentence to a CI log, and only one of them is good news.
#[derive(Debug, Error)]
pub enum TrippyWallError {
    /// A directory that holds sources could not be listed.
    #[error("cannot list {} while collecting the sources: {source}", dir.display())]
    ReadDir {
        /// The directory that could not be listed.
        dir: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A source file could not be read from disk.
    #[error("cannot read the source {}: {source}", path.display())]
    ReadSource {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A source file was read but is not valid Rust.
    #[error(
        "cannot parse {} as Rust: {message}; a file the guard cannot parse is a file \
         whose trippy paths it cannot see",
        path.display()
    )]
    Unparsable {
        /// The file that failed to parse.
        path: PathBuf,
        /// What the parser said.
        message: String,
    },

    /// A source directory holds no Rust file at all.
    #[error(
        "{} holds no Rust source; refusing to report a wall intact when nothing was examined",
        dir.display()
    )]
    NoSources {
        /// The directory that holds no source.
        dir: PathBuf,
    },
}

/// The two ways one file breaks the wall.
///
/// The distinction exists for the remedy. Both faults put a trippy type where
/// an upgrade can reach it, and the two answers are opposites: one file must
/// give its code to the tracer, and the other must stop giving the tracer's
/// names away.
#[derive(Debug, Clone, Copy)]
enum Offense {
    /// The file is not the allowed module, and it names a trippy type.
    Names,
    /// The file is the allowed module, and a `use` inside it carries a trippy
    /// type out to the rest of the crate.
    ReExports,
}

impl Offense {
    /// What the file did, as the verb of the line that reports it.
    fn verb(self) -> &'static str {
        match self {
            Self::Names => "names",
            Self::ReExports => "re-exports",
        }
    }

    /// What the author of the file must do, in one sentence.
    fn remedy(self, allowed_module: &str) -> String {
        match self {
            Self::Names => format!(
                "Move that code into {allowed_module}, and give the caller a type this crate owns."
            ),
            Self::ReExports => format!(
                "Do not re-export a trippy type out of {allowed_module}; give the caller a type \
                 this crate owns."
            ),
        }
    }
}

/// One source file that puts a trippy type where an upgrade of the trippy
/// crates can reach it.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The offending file.
    path: PathBuf,
    /// Which of the two faults this file hit.
    offense: Offense,
    /// The trippy paths at stake, sorted and deduplicated.
    trippy_paths: Vec<String>,
}

impl Offender {
    /// The offending file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The trippy paths at stake, sorted and deduplicated.
    ///
    /// For a file outside the allowed module, these are the paths it names. For
    /// the allowed module, these are the paths its `use` items carry out. The
    /// failure message carries them either way, so a reader knows which names
    /// are at stake without opening the file first.
    #[must_use]
    pub fn trippy_paths(&self) -> &[String] {
        &self.trippy_paths
    }
}

/// The verdict of one audit: which files were examined, and which of them broke
/// the wall.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing. It also means one
/// place decides which of the two remedies each offender gets.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every file the audit read and parsed, sorted by path.
    files: Vec<PathBuf>,
    /// The files that broke the wall, sorted by path.
    offenders: Vec<Offender>,
    /// The module the audit let through, for the message.
    allowed_module: String,
}

impl Report {
    /// True when no examined file names a trippy type outside the allowed
    /// module, and the allowed module re-exports none.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The files that broke the wall, sorted by path.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// How many files the audit read and parsed.
    ///
    /// A caller must assert this is non-zero: a guard that reads nothing
    /// reports clean for the wrong reason.
    #[must_use]
    pub fn files_examined(&self) -> usize {
        self.files.len()
    }

    /// Every file the audit read and parsed, sorted by path.
    ///
    /// [`files_examined`](Self::files_examined) is the size of this set, and
    /// this is the set itself. The difference matters because the guard walks
    /// the tree with its own rules. A perfect matcher pointed at the wrong
    /// directory reports clean with the same silence as a broken matcher, so a
    /// caller that wants to prove the guard looked in the right place compares
    /// these paths against an independent enumeration of the same tree.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Examined {} source files; only {} names a trippy type.",
                self.files.len(),
                self.allowed_module
            );
        }

        writeln!(
            f,
            "{} of {} source files break the wall around {}.",
            self.offenders.len(),
            self.files.len(),
            self.allowed_module
        )?;
        writeln!(
            f,
            "The trippy crates do not promise a stable API, so {} is the one module that an \
             upgrade of them can break.",
            self.allowed_module
        )?;

        // The remedy sits under the offender it answers, rather than once at
        // the end, because the two faults take opposite answers and a reader
        // must not have to work out which one is theirs.
        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} {}: {}",
                offender.path.display(),
                offender.offense.verb(),
                offender.trippy_paths.join(", ")
            )?;
            writeln!(f, "    {}", offender.offense.remedy(&self.allowed_module))?;
        }

        Ok(())
    }
}

/// Audit the real repository: every directory of `krt`, with `trace.rs` the one
/// module that names a trippy type and re-exports none.
///
/// One [`Report`] comes back for the whole package, so a caller reads one count
/// of files examined and one list of offenders however many directories the
/// package grows.
///
/// # Errors
///
/// Returns [`TrippyWallError`] — never a clean [`Report`] — when any one
/// directory cannot be read with confidence. See [`audit_sources`], which does
/// the work for one directory.
///
/// A directory in the table that no longer exists, or that holds no Rust file,
/// is a refusal rather than a smaller audit. The table is the guard's model of
/// the package, and a model that has fallen behind the package is not a model
/// to report a clean verdict from.
pub fn audit(repo_root: &Path) -> Result<Report, TrippyWallError> {
    let mut files = Vec::new();
    let mut offenders = Vec::new();

    for directory in &KRT_DIRECTORIES {
        let report = audit_directory(&repo_root.join(directory.path), directory.allowed_module)?;
        files.extend(report.files);
        offenders.extend(report.offenders);
    }

    files.sort();
    offenders.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(Report {
        files,
        offenders,
        // Both remedies point at the same file wherever the offender sits: the
        // tracer is the one file that names a trippy type, and the one file
        // that must not pass one on.
        allowed_module: TRACE_MODULE.to_owned(),
    })
}

/// Audit the Rust sources under `src_dir`, with `allowed_module` the one module
/// that names a trippy type.
///
/// `allowed_module` is a file name such as `trace.rs`. The permission covers
/// both that file directly under `src_dir` and every file under a directory of
/// the same name beside it, so a later split of the module into submodules does
/// not open the wall without a word.
///
/// The permission is to *name* a trippy type, not to pass one on: a `use` in
/// the allowed module that carries a trippy path out under a visibility of
/// `pub`, `pub(crate)`, `pub(super)`, or `pub(in path)` is an offender like any
/// other. A private `use` is not.
///
/// # Errors
///
/// Returns [`TrippyWallError`] — never a clean [`Report`] — when:
///
/// - a directory cannot be listed ([`ReadDir`](TrippyWallError::ReadDir));
/// - a file cannot be read ([`ReadSource`](TrippyWallError::ReadSource));
/// - a file is not valid Rust ([`Unparsable`](TrippyWallError::Unparsable));
/// - the directory holds no Rust file at all
///   ([`NoSources`](TrippyWallError::NoSources)).
pub fn audit_sources(src_dir: &Path, allowed_module: &str) -> Result<Report, TrippyWallError> {
    audit_directory(src_dir, Some(allowed_module))
}

/// Audit one directory, where `allowed_module` is `None` for a directory in
/// which no file may name a trippy type.
///
/// A caller that names a directory always names the module that may hold a
/// trippy type, so [`audit_sources`] takes a plain name. The absent case
/// belongs to `KRT_DIRECTORIES`, where the directory of integration tests
/// carries no such module at all.
fn audit_directory(
    src_dir: &Path,
    allowed_module: Option<&str>,
) -> Result<Report, TrippyWallError> {
    let files = rust_sources(src_dir)?;
    if files.is_empty() {
        return Err(TrippyWallError::NoSources {
            dir: src_dir.to_path_buf(),
        });
    }

    let mut offenders = Vec::new();
    for path in &files {
        // Every file is read and parsed, the allowed module included. A file
        // the guard skips is a file it cannot vouch for, and the count it
        // reports must be the count of files it truly read.
        let text = fs::read_to_string(path).map_err(|source| TrippyWallError::ReadSource {
            path: path.clone(),
            source,
        })?;
        let file = syn::parse_file(&text).map_err(|error| TrippyWallError::Unparsable {
            path: path.clone(),
            message: error.to_string(),
        })?;

        // Two rules, one for each side of the wall. Outside the allowed module,
        // naming a trippy type at all is the fault. Inside it, naming one is
        // the whole job, and handing that name to the rest of the crate is the
        // fault.
        let offense = if is_allowed_module(src_dir, path, allowed_module) {
            Offense::ReExports
        } else {
            Offense::Names
        };

        let found = trippy_paths(&file, offense);
        if found.is_empty() {
            continue;
        }
        offenders.push(Offender {
            path: path.clone(),
            offense,
            trippy_paths: found.into_iter().collect(),
        });
    }

    Ok(Report {
        files,
        offenders,
        // A directory with no exemption still points a reader at the tracer,
        // because moving the code there is still the remedy.
        allowed_module: allowed_module.unwrap_or(TRACE_MODULE).to_owned(),
    })
}

/// True when `path` is the one module the wall lets name a trippy type: the
/// file `allowed_module` directly under `src_dir`, or any file under a
/// directory of the same name beside it.
///
/// The answer picks which of the two rules the file is read under, rather than
/// whether it is read at all. Every file is read.
///
/// The directory half is deliberate. A later split of the tracer into
/// submodules — `trace/mod.rs` beside `trace/probe.rs` — must not open the wall
/// without a word.
///
/// `None` names no such module, which is what a directory of integration tests
/// gets: the permission belongs to one file in one directory, not to a file
/// name.
fn is_allowed_module(src_dir: &Path, path: &Path, allowed_module: Option<&str>) -> bool {
    let Some(allowed_module) = allowed_module else {
        return false;
    };
    let Ok(relative) = path.strip_prefix(src_dir) else {
        return false;
    };
    if relative == Path::new(allowed_module) {
        return true;
    }
    let Some(directory) = Path::new(allowed_module).file_stem() else {
        return false;
    };
    matches!(
        relative.components().next(),
        Some(Component::Normal(first)) if first == directory
    )
}

/// Every Rust source under `dir`, at any depth, sorted by path.
///
/// A directory that exists and cannot be listed is a refusal. To walk past it
/// would drop files from the audit and report the wall intact for the wrong
/// reason.
fn rust_sources(dir: &Path) -> Result<Vec<PathBuf>, TrippyWallError> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current).map_err(|source| TrippyWallError::ReadDir {
            dir: current.clone(),
            source,
        })?;
        for entry in entries {
            let path = entry
                .map_err(|source| TrippyWallError::ReadDir {
                    dir: current.clone(),
                    source,
                })?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == RS) {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

/// The trippy paths one file puts within reach of an upgrade, under the rule
/// that applies to that file.
///
/// One walk answers each rule, and this is the one door to both. A caller
/// therefore states which side of the wall the file sits on, and never which
/// visitor reads it.
fn trippy_paths(file: &syn::File, offense: Offense) -> BTreeSet<String> {
    match offense {
        Offense::Names => {
            let mut finder = Finder::default();
            finder.visit_file(file);
            finder.found
        }
        Offense::ReExports => {
            let mut finder = ReExportFinder::default();
            finder.visit_file(file);
            finder.found
        }
    }
}

/// The trippy paths one file names, collected by one walk of its syntax tree.
///
/// The four checks are the four shapes the same fact arrives in. See the module
/// header for why each one needs its own visit.
#[derive(Debug, Default)]
struct Finder {
    /// What the walk found, sorted and deduplicated by the set itself.
    found: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Finder {
    /// Check 1: a path whose first segment names a trippy crate.
    fn visit_path(&mut self, node: &'ast syn::Path) {
        if let Some(first) = node.segments.first() {
            if names_trippy(&first.ident) {
                self.found.insert(render_path(node));
            }
        }
        syn::visit::visit_path(self, node);
    }

    /// Check 2: a `use` tree rooted at a trippy crate.
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        collect_use_tree(&node.tree, &mut Vec::new(), &mut self.found);
        syn::visit::visit_item_use(self, node);
    }

    /// Check 3: `extern crate trippy_...;`, which holds a bare identifier.
    fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
        if names_trippy(&node.ident) {
            self.found.insert(node.ident.to_string());
        }
        syn::visit::visit_item_extern_crate(self, node);
    }

    /// Check 4: an identifier in the unparsed body of a macro.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        scan_tokens(&node.tokens, &mut self.found);
        syn::visit::visit_macro(self, node);
    }

    /// Check 4 again, for the other place `syn` keeps unparsed tokens: the
    /// arguments of an attribute, such as `#[derive(trippy_core::Thing)]`.
    fn visit_meta_list(&mut self, node: &'ast syn::MetaList) {
        scan_tokens(&node.tokens, &mut self.found);
        syn::visit::visit_meta_list(self, node);
    }
}

/// The trippy paths one file carries *out* of itself, collected by one walk of
/// its syntax tree.
///
/// This is the walk that reads the allowed module, and it is deliberately
/// narrow. Everything that module names privately is its own business — that is
/// the permission the wall grants it — so the only question left is what leaves
/// it. A `use` is the one item that can hand a name from another crate on, so a
/// `use` is the one item this walk reads.
#[derive(Debug, Default)]
struct ReExportFinder {
    /// What the walk found, sorted and deduplicated by the set itself.
    found: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for ReExportFinder {
    /// A `use` tree rooted at a trippy crate, under a visibility that reaches
    /// past the module which writes it.
    ///
    /// [`collect_use_tree`] computes the rooted paths, so the alias in
    /// `pub(crate) use trippy_core::Port as LeakedPort;` hides nothing: the
    /// path recorded is the one the trippy crate owns.
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        if leaves_the_module(&node.vis) {
            collect_use_tree(&node.tree, &mut Vec::new(), &mut self.found);
        }
        syn::visit::visit_item_use(self, node);
    }
}

/// True when a visibility carries the name it marks past the module that writes
/// it.
///
/// `pub(self)` is the one restriction that reaches nothing. It is the long
/// spelling of private, and no module outside the one that writes it can name
/// what it marks, so a `pub(self) use trippy_core::Port;` in the tracer leaks
/// nothing and does not fire.
///
/// Every other restriction is read as leaving, `pub(in crate::trace)` included,
/// although that one also reaches no further than the tracer. To tell it apart
/// needs the path resolved against the module tree of the whole crate, and the
/// guard holds one file at a time. The over-match is the safe direction: it
/// fails loudly on a line somebody wrote on purpose, where an under-match
/// reports the wall intact for years.
fn leaves_the_module(visibility: &syn::Visibility) -> bool {
    match visibility {
        syn::Visibility::Inherited => false,
        syn::Visibility::Public(_) => true,
        syn::Visibility::Restricted(restricted) => !restricted.path.is_ident("self"),
    }
}

/// True when an identifier starts with the name every trippy crate starts with.
fn names_trippy(ident: &syn::Ident) -> bool {
    ident.to_string().starts_with(TRIPPY)
}

/// Render a path the way a person writes it: the segment identifiers joined
/// with `::`.
///
/// A leading `::` is dropped, and so are the generic arguments of a segment.
/// The result names the item, which is what a reader needs to find it.
fn render_path(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

/// Collect the full paths one `use` tree declares, and keep the ones rooted at
/// a trippy crate.
///
/// `prefix` holds the segments above `tree`. A group divides one tree into
/// several, and each branch of it gets the same prefix.
fn collect_use_tree(tree: &syn::UseTree, prefix: &mut Vec<String>, found: &mut BTreeSet<String>) {
    match tree {
        syn::UseTree::Path(node) => {
            prefix.push(node.ident.to_string());
            collect_use_tree(&node.tree, prefix, found);
            prefix.pop();
        }
        syn::UseTree::Name(node) => record_use_path(prefix, &node.ident.to_string(), found),
        // `use trippy_core as tc;` is the shape the alias hides. The renamed
        // identifier is the one that names the crate, so the rename is read and
        // the new name is not.
        syn::UseTree::Rename(node) => record_use_path(prefix, &node.ident.to_string(), found),
        syn::UseTree::Glob(_) => record_use_path(prefix, "*", found),
        syn::UseTree::Group(node) => {
            for branch in &node.items {
                collect_use_tree(branch, prefix, found);
            }
        }
    }
}

/// Record `prefix::leaf` when the first name in it is a trippy crate.
///
/// The first name is the root of the tree, which is the only segment that names
/// a crate. `use crate::trippy_helper::thing;` is rooted at `crate` and names
/// nothing outside this workspace.
fn record_use_path(prefix: &[String], leaf: &str, found: &mut BTreeSet<String>) {
    let root = prefix.first().map_or(leaf, String::as_str);
    if !root.starts_with(TRIPPY) {
        return;
    }
    let mut segments: Vec<&str> = prefix.iter().map(String::as_str).collect();
    segments.push(leaf);
    found.insert(segments.join(SEPARATOR));
}

/// Record every identifier that names a trippy crate inside a token stream.
///
/// `syn` hands a macro body over as tokens that it did not parse, so no visit
/// of the syntax tree reaches into one. The walk goes into every group and
/// reads an identifier only. It never reads a literal, so the string
/// `"trippy"` in a macro body does not fire.
///
/// A group holds another stream, so the walk keeps its own stack of streams.
/// Recursion here would put the depth of a macro body on the call stack, and a
/// generated file can nest one as deep as it likes.
fn scan_tokens(tokens: &TokenStream, found: &mut BTreeSet<String>) {
    let mut pending = vec![tokens.clone()];

    while let Some(stream) = pending.pop() {
        for token in stream {
            match token {
                TokenTree::Ident(ident) => {
                    let name = ident.to_string();
                    if name.starts_with(TRIPPY) {
                        found.insert(name);
                    }
                }
                TokenTree::Group(group) => pending.push(group.stream()),
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }
}
