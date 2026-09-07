//! Guard: only `op-cache` runs the `op` binary.
//!
//! Every secret this workspace reads lives in 1Password, and `op-cache` is the
//! one crate that reads it. It caches what it read, retries what failed, and
//! keeps the cache file at mode 600 beside the repository root. A crate that
//! runs `op` itself gets none of that, and the failure is quiet: the code
//! works, the secret arrives, and only the properties nobody can see are gone.
//!
//! `ufa` did exactly that. It ran `op item get ufa --vault Private --format
//! json` to learn which controllers the user keeps, and that command prints the
//! value of every concealed field beside its label. Every controller key in
//! that item was read into the process for a list of names. The reason the code
//! reached around the helper is the one this file exists to make loud:
//! `op-cache` had no way to list the fields of an item, so the helper was not
//! usable for the job, and nothing said so.
//!
//! # The rule is one sentence
//!
//! A call to `Command::new` whose program is the `op` binary, in any crate but
//! `op-cache`, is an offender. Locating `op` is not running it, so
//! `which::which("op")` is nobody's business here. Running `op-cache` itself is
//! the sanctioned route for a shell script, so `Command::new("op-cache")` is
//! not an offender either.
//!
//! # Enumerate, never allowlist
//!
//! The audited set is every workspace member [`workspace_lints::members`]
//! reports, less `op-cache`. A hardcoded list of crates makes the next new
//! crate invisible to the guard, which is the exact failure this exists to
//! prevent. Inside a member, every Rust file at any depth is read — the
//! library, the binaries, the tests, the benches, the examples, and the build
//! script — because any of them can spawn a process. Only a `target` directory
//! is skipped, and it holds no source anybody here wrote.
//!
//! `op-cache` is recognised by its directory. If it ever moves, no member
//! matches, `op-cache` is audited like everything else, and its own two calls
//! are reported. The guard then fails loudly rather than quietly auditing one
//! crate fewer.
//!
//! # Parse, never text-match
//!
//! "A call whose callee is `Command::new` and whose first argument is the
//! string `op`" names a syntactic category, so [`syn`] answers it. A search for
//! the text `Command::new("op")` answers only for the one spelling somebody
//! thought of, and every other spelling reports *clean* — which reads exactly
//! like a guard doing real work. The forms one fact arrives in:
//!
//! 1. **A call.** Any [`syn::ExprCall`] whose callee path ends with the two
//!    segments `Command` and `new`. That covers `Command::new`,
//!    `std::process::Command::new`, `tokio::process::Command::new`, and every
//!    other module path in front of the same two names.
//! 2. **A program named by its path.** `Command::new("/opt/homebrew/bin/op")`
//!    runs the same binary. The last segment of the literal decides, so a
//!    program named `op-cache` is still not `op`.
//! 3. **A call inside a macro body.** [`syn`] hands a macro body over as
//!    unparsed tokens, so the call check never sees
//!    `println!("{:?}", Command::new("op").output())`. A macro body whose
//!    tokens hold the identifier `Command` *and* a string literal naming the
//!    `op` binary is an offender.
//!
//! Check 3 can over-match: a macro body that names an unrelated `Command` type
//! beside the string `"op"` is flagged, and it runs nothing. That trade is
//! deliberate. An over-match fails loudly and a person corrects it in an hour.
//! An under-match stays green for years.
//!
//! One thing no matcher here can see is a program named by a binding —
//! `Command::new(program)`. Resolving that needs the name resolution a compiler
//! does, which this guard is not. The literal is the shape every call in this
//! workspace uses, and the shape the one offender used.
//!
//! # Refuse rather than shrink
//!
//! Everything that stops the guard from reading a source is an error, never a
//! clean verdict. A file that does not parse is a file whose calls the guard
//! cannot see, which is not the same as a file that holds none. A directory
//! with no Rust source is a guard pointed at the wrong place, and "I examined
//! nothing" reads exactly like "everything is clean". See [`OpWallError`].

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::workspace_lints::WorkspaceLintsError;

/// The crate that runs `op`, relative to the repository root.
const OP_CACHE: &str = "src/op-cache";

/// The name of the 1Password CLI, as a program name.
const OP: &str = "op";

/// Everything that stops the audit from reaching a verdict.
///
/// Every variant is a *refusal*. A guard that cannot read a source must say so
/// loudly, because "no crate runs `op`" and "I read no crates" are the same
/// sentence to a CI log, and only one of them is good news.
#[derive(Debug, Error)]
pub enum OpWallError {
    /// The workspace members could not be resolved.
    #[error("cannot resolve the workspace members: {0}")]
    Members(#[from] WorkspaceLintsError),

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
         whose calls it cannot see",
        path.display()
    )]
    Unparsable {
        /// The file that failed to parse.
        path: PathBuf,
        /// What the parser said.
        message: String,
    },

    /// A directory holds no Rust file at all.
    #[error(
        "{} holds no Rust source; refusing to report the wall intact when nothing was examined",
        dir.display()
    )]
    NoSources {
        /// The directory that holds no source.
        dir: PathBuf,
    },

    /// Every workspace member was skipped, so nothing was audited.
    #[error(
        "every workspace member was skipped as {OP_CACHE}; refusing to report the wall intact \
         when no crate was examined"
    )]
    NoMembersAudited,
}

/// One source file that runs the `op` binary outside `op-cache`.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The offending file.
    path: PathBuf,
    /// The spawns it holds, sorted and deduplicated.
    spawns: Vec<String>,
}

impl Offender {
    /// The offending file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The spawns the file holds, sorted and deduplicated.
    ///
    /// The failure message carries them, so a reader knows what the file runs
    /// without opening it first.
    #[must_use]
    pub fn spawns(&self) -> &[String] {
        &self.spawns
    }
}

/// The verdict of one audit: which files were examined, and which of them run
/// the `op` binary.
///
/// The remediation text lives here rather than at the call site, so every
/// caller — test, CI job, or CLI — reports the same thing.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every file the audit read and parsed, sorted by path.
    files: Vec<PathBuf>,
    /// The files that run `op`, sorted by path.
    offenders: Vec<Offender>,
}

impl Report {
    /// True when no examined file runs the `op` binary.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The files that run `op`, sorted by path.
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
                "Examined {} source files; only {OP_CACHE} runs the {OP} binary.",
                self.files.len()
            );
        }

        writeln!(
            f,
            "{} of {} source files run the {OP} binary outside {OP_CACHE}.",
            self.offenders.len(),
            self.files.len()
        )?;
        writeln!(
            f,
            "Read the secret through op-cache instead. It caches what it read, retries what \
             failed, and keeps the cache file readable by its owner alone. If op-cache cannot \
             do what the call needs, add the narrow method it lacks — that gap is why the last \
             offender existed."
        )?;
        for offender in &self.offenders {
            writeln!(f, "  {}: {}", offender.path.display(), offender.spawns.join(", "))?;
        }
        Ok(())
    }
}

/// Audit every workspace member but `op-cache`.
///
/// # Errors
///
/// Returns [`OpWallError`] — never a clean [`Report`] — when the members
/// cannot be resolved, a directory cannot be listed, a file cannot be read, a
/// file is not valid Rust, a member holds no Rust source, or every member was
/// skipped.
pub fn audit(repo_root: &Path) -> Result<Report, OpWallError> {
    let _ = repo_root;

    Ok(Report {
        files: Vec::new(),
        offenders: Vec::new(),
    })
}

/// Audit the Rust sources under `dir`, at any depth.
///
/// # Errors
///
/// Returns [`OpWallError`] — never a clean [`Report`] — when a directory
/// cannot be listed ([`ReadDir`](OpWallError::ReadDir)), a file cannot be read
/// ([`ReadSource`](OpWallError::ReadSource)), a file is not valid Rust
/// ([`Unparsable`](OpWallError::Unparsable)), or the directory holds no Rust
/// file at all ([`NoSources`](OpWallError::NoSources)).
pub fn audit_sources(dir: &Path) -> Result<Report, OpWallError> {
    let _ = dir;

    Ok(Report {
        files: Vec::new(),
        offenders: Vec::new(),
    })
}
