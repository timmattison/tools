//! The second pass: a `#[cfg(test)] mod <name>;` declaration marks the file it
//! names.
//!
//! This is the one rule of the tool that reads across files. A Rust file that
//! declares
//!
//! ```text
//! #[cfg(test)]
//! mod tests;
//! ```
//!
//! holds none of the test code it is talking about: the whole of the file it
//! names is test code. No path rule can see that, because the name of the named
//! file proves nothing on its own — `tests.rs` is an ordinary name — and no
//! tree rule can see it either, because the evidence lives in a different file.
//! So the tree rule collects the name while it has the declaring file open, and
//! [`resolve_test_modules`] resolves it once every file has been counted and
//! before the counts are added up.
//!
//! # Where the file actually is
//!
//! The two candidates are `<module directory>/<name>.rs` and `<module
//! directory>/<name>/mod.rs`, and the module directory is the part worth
//! stating, because it is *not* always the directory the declaring file lives
//! in:
//!
//! | Declaring file | Declaration | Names |
//! | --- | --- | --- |
//! | `src/lib.rs` | `mod tests;` | `src/tests.rs`, `src/tests/mod.rs` |
//! | `src/main.rs` | `mod tests;` | `src/tests.rs`, `src/tests/mod.rs` |
//! | `src/foo/mod.rs` | `mod bar;` | `src/foo/bar.rs`, `src/foo/bar/mod.rs` |
//! | `src/foo.rs` | `mod bar;` | `src/foo/bar.rs`, `src/foo/bar/mod.rs` |
//!
//! The last row is the one a simpler rule gets wrong in both directions: a
//! module declared in `src/foo.rs` lives under `src/foo/`, so "the same
//! directory" would miss `src/foo/bar.rs` and mark `src/bar.rs`, which belongs
//! to a different module and may well be production code. `mod.rs`, `lib.rs`,
//! and `main.rs` are the three names whose module directory is their own
//! parent, and every other file adds its own stem.
//!
//! A `#[path = "…"] mod x;` names a file directly and is out of scope. Such a
//! declaration marks the two rows of itself, as the tree rule marked them, and
//! nothing else.
//!
//! # What the pass will not do
//!
//! **It never touches the filesystem.** A candidate is matched against the
//! paths the walk already produced, so a target that the walk never visited —
//! one outside the roots, or one `.gitignore` excluded — is silently nothing,
//! rather than a file the counter reads behind the walk's back.
//!
//! **It never moves a file twice.** A file that is already wholly test material
//! is left exactly as it is, whichever rule put it there. That is what makes
//! the pass idempotent, and it is why two files declaring one target, or one
//! target that a glob had already marked, still carry one span and not two.

use crate::file::{FileCount, Rule, Span};
use crate::lines::Counts;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// The stems of the three files whose module directory is the directory they
/// live in. Every other file adds its own stem to that directory.
const ROOT_STEMS: &[&str] = &["mod", "lib", "main"];

/// The extension of a Rust source file.
const RUST_EXTENSION: &str = "rs";

/// The file that is the root of a module written as a directory.
const MOD_FILE: &str = "mod.rs";

/// Mark every file that a `#[cfg(test)] mod <name>;` declaration names.
///
/// This is the one rule of the tool that reads across files, so it runs after
/// every file has been counted and before the counts are added up.
///
/// A file the pass marks moves wholly into the test bucket and gains one
/// [`Rule::ModDeclaration`] span naming the module, so `--explain` can say
/// which declaration moved it. A file no declaration names is left untouched,
/// and so is one that is already wholly test material — see the module doc.
pub fn resolve_test_modules(files: &mut [FileCount]) {
    let mut by_path: HashMap<PathBuf, usize> = HashMap::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        by_path.entry(normalized(&file.path)).or_insert(index);
    }

    // The targets are collected before any of them is marked, because a
    // declaration is read through a shared borrow of the files and marking one
    // needs an exclusive borrow of the same slice.
    let mut targets: Vec<(usize, String)> = Vec::new();
    for file in files.iter() {
        for module in &file.test_mod_declarations {
            for candidate in candidates(&file.path, module) {
                if let Some(&index) = by_path.get(&normalized(&candidate)) {
                    targets.push((index, module.clone()));
                }
            }
        }
    }

    for (index, module) in targets {
        if let Some(file) = files.get_mut(index) {
            mark_as_test(file, &module);
        }
    }
}

/// The files a declaration of `module` in `declaring` could name.
///
/// Both spellings of a module are offered, because only the filesystem knows
/// which one is there and this pass does not ask it: the answer is whichever of
/// the two the walk found. A tree holding both is a tree that does not compile,
/// and marking both is the honest reading of it.
fn candidates(declaring: &Path, module: &str) -> [PathBuf; 2] {
    let directory = module_directory(declaring);
    [
        directory.join(format!("{module}.{RUST_EXTENSION}")),
        directory.join(module).join(MOD_FILE),
    ]
}

/// The directory the modules of `declaring` live in. See the module doc.
fn module_directory(declaring: &Path) -> PathBuf {
    let parent = declaring
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf();
    let stem = declaring.file_stem().unwrap_or_else(|| OsStr::new(""));
    if ROOT_STEMS.iter().any(|root| stem == OsStr::new(root)) {
        parent
    } else {
        parent.join(stem)
    }
}

/// A path with every `.` component dropped.
///
/// A walk of `.` yields `./src/lib.rs` and a walk of `src` yields `src/lib.rs`,
/// and a candidate built from either one has to match the other. Both sides go
/// through this, so the comparison is between two paths spelled the same way.
///
/// It is deliberately not [`std::fs::canonicalize`]: a candidate is a path that
/// may not exist, canonicalising it would fail rather than answer, and asking
/// the filesystem about a file the walk never visited is exactly what this pass
/// must not do. A `..` component is left alone for the same reason — resolving
/// one lexically is wrong wherever a symbolic link is involved, and both sides
/// carry the same prefix anyway, because both came from the same walk.
fn normalized(path: &Path) -> PathBuf {
    path.components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect()
}

/// Moves a whole file into the test bucket under one declaration span.
///
/// A file that holds no production row is left exactly as it is. That covers
/// three cases with one rule: a file this pass has already marked, so running
/// the pass twice says what running it once said; a file a glob or the tree
/// rule had already marked whole, which keeps the one span that names the rule
/// that really decided it; and a file of no rows at all, which gains no span,
/// because a span over nothing is a region spelled as no region.
fn mark_as_test(file: &mut FileCount, module: &str) {
    if file.production.total() == 0 {
        return;
    }

    let rows = u32::try_from(file.total().total()).unwrap_or(u32::MAX);
    file.test = file.total();
    file.production = Counts::default();
    file.spans.push(Span {
        first_row: 1,
        last_row: rows,
        rule: Rule::ModDeclaration(module.to_string()),
    });
}
