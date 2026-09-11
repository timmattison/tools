//! Guard: no source file in this workspace removes a `GIT_` variable by name.
//!
//! A tool that spawns git — or spawns something that runs git — hands the child
//! whatever `GIT_*` variables it inherited. Git obeys those before it obeys the
//! directory the command was pointed at, so a run started from a pre-commit
//! hook aims its children at the repository being committed to. That defect
//! arrived one call site at a time, and each arrival was repaired the same way:
//! a short list of the names somebody had thought of.
//!
//! [`gitscratch::shed_inherited_git_environment`] holds the rule that replaces
//! the list. It enumerates the process environment and removes every key with
//! the `GIT_` prefix, so a variable git invents next year leaves without anyone
//! editing a file. Its own documentation states the gap this module closes:
//!
//! > That is an offer, not a guarantee. Nothing - no lint, no type, no guard -
//! > obliges a git spawn in this repository to call this, so immunity holds
//! > where it is called and nowhere else.
//!
//! # The rule
//!
//! A call to `env_remove` whose argument holds a string literal starting with
//! `GIT_` is a violation, wherever it appears. That is the shape of a named
//! list, and it is the only shape [`audit`] judges.
//!
//! The shared sweep passes a key it read from the environment, never a literal,
//! so it never trips its own rule. Neither does a removal of a variable that is
//! not git's.
//!
//! # Why a list is the defect rather than a style
//!
//! `~/.claude/HERMETIC-TESTS.md` states it as a rule: strip by prefix, never by
//! a list. A list strips nothing new the day git adds a variable, and from then
//! on it returns the same clean-looking answer as a list that works. Two
//! variables walked through the three-name list this workspace kept copying.
//! `GIT_OBJECT_DIRECTORY` redirects a child's object writes into another
//! repository's store. `GIT_CONFIG_PARAMETERS`, which git exports into every
//! hook, injects arbitrary configuration — `user.email`, `core.bare`,
//! `core.hooksPath` — into the child. Neither names a location, so no amount of
//! adding location names would have caught either.
//!
//! # Why a lint cannot say this
//!
//! `clippy.toml` supports `disallowed-methods`, which matches a method path and
//! ignores its arguments. The predicate here is about the argument, so clippy
//! can only ban `env_remove` outright, which fires on every legitimate removal
//! of a variable that is not git's.
//!
//! # Parse to fail closed, then match over tokens
//!
//! Each file is parsed with [`syn::parse_file`] first. That is the fail-closed
//! half: a file this guard cannot read as Rust is an error rather than a clean
//! verdict, because "no violation here" and "I never understood this file"
//! print identically. [`syn`] returns an error rather than recovering silently,
//! so nothing is judged on a partial read.
//!
//! The verdict itself is then taken over the file's [`proc_macro2`] token
//! stream. Tokens rather than the syntax tree, for one reason: [`syn`] leaves
//! the body of every macro invocation as an unparsed token stream, so a rule
//! walking the tree alone cannot see an `env_remove` written inside one. The
//! token stream has no such blind spot, and it keeps the property the tree
//! gives — **prose is data**. A `//` comment never becomes a token at all, a
//! doc comment becomes one string literal, and the text inside a string is one
//! token rather than the words it spells. So a file that merely *names*
//! `env_remove("GIT_DIR")` in a sentence is not a violation, which is the
//! failure mode a text search has and this does not.
//!
//! The matched shape is an identifier `env_remove` followed immediately by a
//! parenthesized group. An identifier followed by `!` is a macro invocation and
//! is not matched. The group is then searched, to any depth, for a string
//! literal starting with `GIT_`, so every spelling of the same call reduces to
//! the same answer: a bare literal, a reference to one, or one built by a macro.
//!
//! # The exemptions
//!
//! [`EXEMPTIONS`] names the removals that are deliberate, one per (file,
//! variable) pair rather than per file, so a *new* named removal in an exempt
//! file is still a violation. Each pair states its reason here and at the site
//! it exempts.
//!
//! An exemption that matches nothing is itself reported, through
//! [`Report::unused_exemptions`]. An allowlist nobody prunes is how a guard
//! quietly stops covering the thing it was written for.
//!
//! # Scope is the sibling defect
//!
//! `~/.claude/REGEX-VS-AST.md` names it: a perfect matcher pointed at four of
//! seven directories reports clean for the same reason and with the same
//! silence. So the scanned set is derived from the workspace manifest, through
//! [`workspace_lints::members`], exactly as the sibling guards derive theirs,
//! and an empty set is an error rather than a clean verdict. A companion test
//! measures that set against the target roots `cargo metadata` reports, so a
//! directory this walk never reaches shows up as a set difference.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use thiserror::Error;
use walkdir::WalkDir;

use crate::workspace_lints::{self, WorkspaceLintsError};

/// The prefix that makes a variable git's.
///
/// The whole rule, spelled once. A guard that carried a list of names here
/// would be the defect it exists to report.
pub const GIT_ENVIRONMENT_PREFIX: &str = "GIT_";

/// The method whose argument this guard reads.
const ENV_REMOVE: &str = "env_remove";

/// Rust source extension.
const RS: &str = "rs";

/// The marker a build tool drops in a directory of generated artifacts.
///
/// Cargo writes one into its target directory. A directory holding it is not
/// source, and walking into it would judge whatever a build happened to leave
/// there.
const CACHEDIR_TAG: &str = "CACHEDIR.TAG";

/// One deliberate removal of a `GIT_` variable by name.
///
/// Keyed on the variable as well as the file, so exempting one removal does not
/// exempt the file that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exemption {
    /// The file, relative to the repository root, spelled with `/` separators.
    pub file: &'static str,
    /// The variable that file removes by name.
    pub variable: &'static str,
    /// Why the removal is deliberate. Stated here and at the site.
    pub reason: &'static str,
}

/// Every removal of a `GIT_` variable by name that is deliberate.
///
/// Both entries are the same decision, made twice: `gitscratch` sweeps the
/// whole prefix and then restates the removal of the two date variables *after*
/// the sweep, so the second guard holds on its own if the sweep is ever edited
/// away. A pinned date would stamp every commit of one run with one identical
/// time, so these two leave rather than get pinned like the four names beside
/// them.
///
/// Public so a test can measure the list against what the repository actually
/// holds. An entry matching nothing is reported by
/// [`Report::unused_exemptions`].
pub const EXEMPTIONS: [Exemption; 2] = [
    Exemption {
        file: "src/gitscratch/src/git.rs",
        variable: "GIT_AUTHOR_DATE",
        reason: "a restated removal after the blanket sweep, so the second guard holds alone",
    },
    Exemption {
        file: "src/gitscratch/src/git.rs",
        variable: "GIT_COMMITTER_DATE",
        reason: "a restated removal after the blanket sweep, so the second guard holds alone",
    },
];

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot read the workspace must
/// say so loudly, because "nothing removes a git variable by name" and "I
/// looked at nothing" are the same sentence to a CI log, and only one of them
/// is good news.
#[derive(Debug, Error)]
pub enum GitEnvSweepError {
    /// The workspace members could not be enumerated.
    #[error("cannot enumerate the workspace members: {0}")]
    Members(#[from] WorkspaceLintsError),

    /// A member directory could not be walked.
    #[error("cannot walk {} while collecting source files: {source}", dir.display())]
    Walk {
        /// The directory the walk gave up in.
        dir: PathBuf,
        /// The underlying failure.
        source: walkdir::Error,
    },

    /// A source file could not be read from disk.
    #[error("cannot read the source file {}: {source}", path.display())]
    ReadSource {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O failure.
        source: io::Error,
    },

    /// A source file was read but is not valid Rust.
    #[error(
        "cannot parse {} as Rust: {source}; a file this guard cannot read is a file it cannot \
         vouch for",
        path.display()
    )]
    ParseSource {
        /// The file that failed to parse.
        path: PathBuf,
        /// The underlying parse failure.
        source: syn::Error,
    },

    /// The walk found no Rust source at all.
    #[error(
        "found no Rust source under the {members} workspace members; refusing to report a \
         workspace clean when nothing in it was read"
    )]
    NoSourceFiles {
        /// How many member directories were walked.
        members: usize,
    },
}

/// One file that removes at least one `GIT_` variable by name.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The file, relative to the repository root.
    path: PathBuf,
    /// The variables it removes by name, sorted and deduplicated.
    variables: Vec<String>,
}

impl Offender {
    /// The offending file, relative to the repository root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `GIT_` variables this file removes by name, sorted.
    #[must_use]
    pub fn variables(&self) -> &[String] {
        &self.variables
    }
}

/// The verdict of one audit: what was read, and which files name a `GIT_`
/// variable in a removal.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or command — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every file read, relative to the repository root and sorted.
    files: Vec<PathBuf>,
    /// Offending files, sorted by path.
    offenders: Vec<Offender>,
    /// The exemptions that matched a removal actually present in the source.
    applied: BTreeSet<(String, String)>,
}

impl Report {
    /// True when no file removes a `GIT_` variable by name.
    ///
    /// An unused exemption is *not* a compliance failure. It is a fact about
    /// the allowlist rather than about the source, so
    /// [`unused_exemptions`](Self::unused_exemptions) reports it separately and
    /// a caller decides how loudly to say so.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The files that remove a `GIT_` variable by name, sorted by path.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// Every file the audit read, relative to the repository root and sorted.
    ///
    /// A caller compares this against another enumeration of the same
    /// workspace — `cargo metadata`, which needs no model because it is the
    /// build — to prove the walk reaches every file cargo compiles.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// How many files the audit read.
    ///
    /// A caller should assert this is non-zero: a guard that reads nothing
    /// reports clean for the wrong reason.
    #[must_use]
    pub fn files_examined(&self) -> usize {
        self.files.len()
    }

    /// The entries of [`EXEMPTIONS`] that matched no removal in the source.
    ///
    /// An exemption that matches nothing has outlived the code it excused. Left
    /// in place it silently widens the guard the day somebody writes that
    /// removal again.
    #[must_use]
    pub fn unused_exemptions(&self) -> Vec<&'static Exemption> {
        EXEMPTIONS
            .iter()
            .filter(|exemption| {
                !self
                    .applied
                    .contains(&(exemption.file.to_owned(), exemption.variable.to_owned()))
            })
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Read {} Rust source files; none removes a {GIT_ENVIRONMENT_PREFIX} variable by name.",
                self.files.len()
            );
        }

        writeln!(
            f,
            "{} of {} Rust source files remove a {GIT_ENVIRONMENT_PREFIX} variable by name.",
            self.offenders.len(),
            self.files.len()
        )?;
        writeln!(
            f,
            "A named list strips nothing new the day git adds a variable, and from then on it \
             returns the same clean-looking answer as a list that works."
        )?;
        writeln!(
            f,
            "Call `gitscratch::shed_inherited_git_environment(&mut command)` instead, which \
             removes every key carrying the {GIT_ENVIRONMENT_PREFIX} prefix."
        )?;

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} removes by name: {}",
                offender.path.display(),
                offender.variables.join(", ")
            )?;
        }

        Ok(())
    }
}

/// Read every Rust source file of the workspace at `repo_root` and report the
/// ones that remove a `GIT_` variable by name.
///
/// # Errors
///
/// Returns [`GitEnvSweepError`] — never a short list — when the workspace
/// cannot be read with confidence: an unenumerable workspace, an unwalkable
/// member directory, an unreadable or unparsable source file, or a walk that
/// finds no Rust at all.
pub fn audit(repo_root: &Path) -> Result<Report, GitEnvSweepError> {
    let files = source_files(repo_root)?;

    let mut offenders = Vec::new();
    let mut applied = BTreeSet::new();

    for relative in &files {
        let named = named_removals(&repo_root.join(relative))?;
        if named.is_empty() {
            continue;
        }

        let key = relative.to_string_lossy().replace('\\', "/");
        let mut variables = Vec::new();
        for variable in named {
            if EXEMPTIONS
                .iter()
                .any(|exemption| exemption.file == key && exemption.variable == variable)
            {
                applied.insert((key.clone(), variable));
                continue;
            }
            variables.push(variable);
        }

        if !variables.is_empty() {
            offenders.push(Offender {
                path: relative.clone(),
                variables,
            });
        }
    }

    Ok(Report {
        files,
        offenders,
        applied,
    })
}

/// Every Rust source file under the workspace members of `repo_root`, relative
/// to `repo_root` and sorted.
///
/// Lifted out of [`audit`] so a test can measure the scanned set against
/// `cargo metadata` without re-deriving it, which is the only way to tell a
/// guard that found nothing from a guard that looked nowhere.
///
/// # Errors
///
/// Returns [`GitEnvSweepError`] when the members cannot be enumerated, a member
/// directory cannot be walked, or the walk finds no Rust source.
pub fn source_files(repo_root: &Path) -> Result<Vec<PathBuf>, GitEnvSweepError> {
    let members = workspace_lints::members(repo_root)?;

    let mut files = Vec::new();
    for dir in &members {
        for entry in WalkDir::new(dir).into_iter().filter_entry(is_source_tree) {
            let entry = entry.map_err(|source| GitEnvSweepError::Walk {
                dir: dir.clone(),
                source,
            })?;
            let path = entry.path();
            if entry.file_type().is_file() && path.extension().is_some_and(|ext| ext == RS) {
                files.push(path.strip_prefix(repo_root).unwrap_or(path).to_path_buf());
            }
        }
    }

    files.sort();
    files.dedup();
    if files.is_empty() {
        return Err(GitEnvSweepError::NoSourceFiles {
            members: members.len(),
        });
    }

    Ok(files)
}

/// False for a directory of generated artifacts, which is not source.
///
/// Cargo marks its target directory with a [`CACHEDIR_TAG`], and so does every
/// other tool that adopted the convention. Asking for the marker rather than
/// for the name `target` means a source directory that happens to be called
/// `target` is still read.
fn is_source_tree(entry: &walkdir::DirEntry) -> bool {
    !entry.file_type().is_dir() || !entry.path().join(CACHEDIR_TAG).is_file()
}

/// The `GIT_` variables `path` removes by name, sorted and deduplicated.
fn named_removals(path: &Path) -> Result<Vec<String>, GitEnvSweepError> {
    let source = fs::read_to_string(path).map_err(|source| GitEnvSweepError::ReadSource {
        path: path.to_path_buf(),
        source,
    })?;

    // The fail-closed half. A file that is not valid Rust is refused rather
    // than read as clean, and `syn` errors rather than recovering silently, so
    // no verdict is taken from a partial read.
    syn::parse_file(&source).map_err(|source| GitEnvSweepError::ParseSource {
        path: path.to_path_buf(),
        source,
    })?;

    let tokens = TokenStream::from_str(&source).map_err(|error| GitEnvSweepError::ParseSource {
        path: path.to_path_buf(),
        source: syn::Error::new(proc_macro2::Span::call_site(), error),
    })?;

    let mut found = BTreeSet::new();
    collect_named_removals(tokens, &mut found);
    Ok(found.into_iter().collect())
}

/// Walk `tokens`, recording every `GIT_` variable removed by name.
///
/// Recursion follows every group, so a call nested in a block, a closure, or
/// the body of a macro invocation is reached the same way a top-level one is.
fn collect_named_removals(tokens: TokenStream, found: &mut BTreeSet<String>) {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();

    for (index, tree) in trees.iter().enumerate() {
        if let TokenTree::Ident(ident) = tree {
            if ident == ENV_REMOVE {
                // An identifier followed immediately by a parenthesized group
                // is a call. `env_remove!(...)` puts a `!` between the two, so
                // a macro of that name is not read as one.
                if let Some(TokenTree::Group(group)) = trees.get(index + 1) {
                    if group.delimiter() == Delimiter::Parenthesis {
                        collect_git_literals(group.stream(), found);
                    }
                }
            }
        }

        if let TokenTree::Group(group) = tree {
            collect_named_removals(group.stream(), found);
        }
    }
}

/// Record every `GIT_`-prefixed string literal in `tokens`, to any depth.
///
/// The whole argument is searched rather than only its first token, so a
/// literal behind a reference, a `const` spelled inline, or a macro that builds
/// the name is found. That over-matches a call that merely mentions such a
/// literal while removing something else, and that is the safe direction: an
/// over-matching guard fails loudly and gets repaired, while one that under-
/// matches reports clean and stays that way.
fn collect_git_literals(tokens: TokenStream, found: &mut BTreeSet<String>) {
    for tree in tokens {
        match tree {
            TokenTree::Literal(literal) => {
                if let syn::Lit::Str(text) = syn::Lit::new(literal) {
                    let value = text.value();
                    if value.starts_with(GIT_ENVIRONMENT_PREFIX) {
                        found.insert(value);
                    }
                }
            }
            TokenTree::Group(group) => collect_git_literals(group.stream(), found),
            TokenTree::Ident(_) | TokenTree::Punct(_) => {}
        }
    }
}
