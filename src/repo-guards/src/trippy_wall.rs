//! Guard: no module of `krt` except `trace.rs` names a type from a trippy
//! crate.
//!
//! `krt` records a network path with the `trippy-core` and `trippy-privilege`
//! crates. The documentation of `trippy-core` says that its public API is not
//! stable, and that it is highly likely to change. Two rules answer that risk.
//! The manifest pins the exact version, and one module — `trace.rs` — is the
//! only place that names a trippy type. Every other module speaks to the tracer
//! through types that `krt` owns. An upgrade of the trippy crates then breaks
//! one file, and the compiler shows the whole break in that one file.
//!
//! The first rule needs no guard, because a pinned version is visible in the
//! manifest. The second rule needs one. A trippy path in the wrong module looks
//! like an ordinary line of code, and nothing fails when a person writes it. So
//! it spreads: the module that needs one field of a trippy struct takes the
//! struct, and the wall is gone before a reviewer reads the diff.
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

/// The source directory of the crate that carries the wall, relative to the
/// repository root.
const KRT_SRC: &str = "src/krt/src";

/// The one module of that crate which names a trippy type.
const TRACE_MODULE: &str = "trace.rs";

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

/// One source file that names a trippy type and is not the allowed module.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The offending file.
    path: PathBuf,
    /// The trippy paths that file names, sorted and deduplicated.
    trippy_paths: Vec<String>,
}

impl Offender {
    /// The offending file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The trippy paths that file names, sorted and deduplicated.
    ///
    /// The failure message carries these so a reader knows what to move,
    /// without opening the file first.
    #[must_use]
    pub fn trippy_paths(&self) -> &[String] {
        &self.trippy_paths
    }
}

/// The verdict of one audit: which files were examined, and which of them name
/// a trippy type outside the allowed module.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every file the audit read and parsed, sorted by path.
    files: Vec<PathBuf>,
    /// The files that name a trippy type outside the allowed module, sorted by
    /// path.
    offenders: Vec<Offender>,
    /// The module the audit let through, for the message.
    allowed_module: String,
}

impl Report {
    /// True when no examined file except the allowed module names a trippy
    /// type.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The files that name a trippy type outside the allowed module, sorted by
    /// path.
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
            "{} of {} source files name a trippy type outside {}.",
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

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} names: {}",
                offender.path.display(),
                offender.trippy_paths.join(", ")
            )?;
        }

        writeln!(f)?;
        write!(
            f,
            "Move that code into {}, and give the caller a type this crate owns.",
            self.allowed_module
        )
    }
}

/// Audit the real repository: the sources of `krt`, with `trace.rs` allowed.
///
/// # Errors
///
/// Returns [`TrippyWallError`] — never a clean [`Report`] — when the sources
/// cannot be read with confidence. See [`audit_sources`], which does the work.
pub fn audit(repo_root: &Path) -> Result<Report, TrippyWallError> {
    audit_sources(&repo_root.join(KRT_SRC), TRACE_MODULE)
}

/// Audit the Rust sources under `src_dir`, with `allowed_module` let through.
///
/// `allowed_module` is a file name such as `trace.rs`. It lets through both
/// that file directly under `src_dir` and every file under a directory of the
/// same name beside it, so a later split of the module into submodules does not
/// open the wall without a word.
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

        if is_allowed(src_dir, path, allowed_module) {
            continue;
        }

        let mut finder = Finder::default();
        finder.visit_file(&file);
        if finder.found.is_empty() {
            continue;
        }
        offenders.push(Offender {
            path: path.clone(),
            trippy_paths: finder.found.into_iter().collect(),
        });
    }

    Ok(Report {
        files,
        offenders,
        allowed_module: allowed_module.to_owned(),
    })
}

/// True when `path` belongs to the one module the wall lets through: the file
/// `allowed_module` directly under `src_dir`, or any file under a directory of
/// the same name beside it.
///
/// The directory half is deliberate. A later split of the tracer into
/// submodules — `trace/mod.rs` beside `trace/probe.rs` — must not open the wall
/// without a word.
fn is_allowed(src_dir: &Path, path: &Path, allowed_module: &str) -> bool {
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
