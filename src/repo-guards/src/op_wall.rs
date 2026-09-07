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
//! script — because any of them can spawn a process. Two directories are
//! skipped, `target` and `fixtures`, and `is_not_a_source_directory` states
//! what each one costs.
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

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use proc_macro2::{Ident, Literal, TokenStream, TokenTree};
use syn::visit::{self, Visit};
use syn::{Expr, ExprCall, File, Lit, Macro};
use thiserror::Error;

use crate::workspace_lints::{self, WorkspaceLintsError};

/// The crate that runs `op`, relative to the repository root.
const OP_CACHE: &str = "src/op-cache";

/// The name of the 1Password CLI, as a program name.
const OP: &str = "op";

/// The type whose constructor starts a child process.
const COMMAND: &str = "Command";

/// The constructor of that type.
const NEW: &str = "new";

/// The extension of a Rust source file.
const RS: &str = "rs";

/// A directory of build output. It holds sources nobody in this workspace
/// wrote, and reading them says nothing about this workspace.
const TARGET: &str = "target";

/// A directory of test data. What it holds is read by a test rather than
/// compiled into one.
const FIXTURES: &str = "fixtures";

/// The separators a program path can carry, on either family of system.
const PATH_SEPARATORS: [char; 2] = ['/', '\\'];

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
            writeln!(
                f,
                "  {}: {}",
                offender.path.display(),
                offender.spawns.join(", ")
            )?;
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
    let mut files = Vec::new();
    let mut offenders = Vec::new();

    for member in audited_members(repo_root)? {
        let report = audit_sources(&member)?;
        files.extend(report.files);
        offenders.extend(report.offenders);
    }

    files.sort();
    offenders.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(Report { files, offenders })
}

/// The members this audit reads: every one but `op-cache`.
///
/// `op-cache` is recognised by its directory. If it moves, no member matches,
/// and its own calls are reported — a loud failure rather than a quiet drop of
/// one crate from the audited set.
fn audited_members(repo_root: &Path) -> Result<Vec<PathBuf>, OpWallError> {
    let members: Vec<PathBuf> = workspace_lints::members(repo_root)?
        .into_iter()
        .filter(|member| !member.ends_with(OP_CACHE))
        .collect();

    if members.is_empty() {
        return Err(OpWallError::NoMembersAudited);
    }

    Ok(members)
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
    let files = rust_sources(dir)?;
    if files.is_empty() {
        return Err(OpWallError::NoSources {
            dir: dir.to_path_buf(),
        });
    }

    let mut offenders = Vec::new();
    for path in &files {
        let text = fs::read_to_string(path).map_err(|source| OpWallError::ReadSource {
            path: path.clone(),
            source,
        })?;
        let file = syn::parse_file(&text).map_err(|error| OpWallError::Unparsable {
            path: path.clone(),
            message: error.to_string(),
        })?;

        let spawns = spawns_of_op(&file);
        if spawns.is_empty() {
            continue;
        }
        offenders.push(Offender {
            path: path.clone(),
            spawns: spawns.into_iter().collect(),
        });
    }

    Ok(Report { files, offenders })
}

/// Every Rust source under `dir`, at any depth, sorted by path.
///
/// A directory that exists and cannot be listed is a refusal. To walk past it
/// would drop files from the audit and report the wall intact for the wrong
/// reason.
fn rust_sources(dir: &Path) -> Result<Vec<PathBuf>, OpWallError> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];

    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current).map_err(|source| OpWallError::ReadDir {
            dir: current.clone(),
            source,
        })?;
        for entry in entries {
            let path = entry
                .map_err(|source| OpWallError::ReadDir {
                    dir: current.clone(),
                    source,
                })?
                .path();
            if path.is_dir() {
                if path.file_name().is_some_and(is_not_a_source_directory) {
                    continue;
                }
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == RS) {
                files.push(path);
            }
        }
    }

    files.sort();
    Ok(files)
}

/// Whether a directory holds something other than the sources of this
/// workspace.
///
/// Two directories qualify, and both are named rather than guessed at, so the
/// hole each one opens is visible to a reader.
///
/// `target` holds build output: sources nobody here wrote, in numbers that
/// would swamp the audit.
///
/// `fixtures` holds test data. A file there is read by a test rather than
/// compiled into one, and this workspace keeps Rust there that is deliberately
/// not valid Rust — `src/cdva/tests/fixtures/rust/syntax_error.rs` is named
/// for what it holds. Reading those as sources refuses every run of the guard,
/// on a fault that is the whole point of the file.
///
/// The cost is stated: a real source under a directory named `fixtures` is
/// invisible to this guard. Nothing else in the walk is skipped, so a source
/// anywhere else — the library, the binaries, the tests, the benches, the
/// examples, the build script — is read.
fn is_not_a_source_directory(name: &std::ffi::OsStr) -> bool {
    name == TARGET || name == FIXTURES
}

/// Every spawn of the `op` binary one parsed file holds, sorted and
/// deduplicated.
fn spawns_of_op(file: &File) -> BTreeSet<String> {
    let mut spawns = Spawns {
        found: BTreeSet::new(),
    };
    spawns.visit_file(file);
    spawns.found
}

/// The walk that reads one file.
struct Spawns {
    /// What the file runs, rendered for the report.
    found: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Spawns {
    fn visit_expr_call(&mut self, call: &'ast ExprCall) {
        if let Some(spawn) = spawn_of_op(call) {
            self.found.insert(spawn);
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, macro_call: &'ast Macro) {
        if let Some(spawn) = spawn_inside_tokens(&macro_call.tokens) {
            self.found.insert(spawn);
        }
        visit::visit_macro(self, macro_call);
    }
}

/// The spawn of `op` a call holds, when it holds one.
///
/// The callee is read from the end, so every module path in front of the two
/// segments `Command` and `new` reads the same: `Command::new`,
/// `std::process::Command::new`, and `tokio::process::Command::new`.
fn spawn_of_op(call: &ExprCall) -> Option<String> {
    let Expr::Path(callee) = call.func.as_ref() else {
        return None;
    };
    let mut segments = callee.path.segments.iter().rev();
    if segments.next()?.ident != NEW || segments.next()?.ident != COMMAND {
        return None;
    }

    let Some(Expr::Lit(literal)) = call.args.first() else {
        return None;
    };
    let Lit::Str(program) = &literal.lit else {
        return None;
    };
    let program = program.value();

    names_the_op_binary(&program).then(|| format!("{COMMAND}::{NEW}(\"{program}\")"))
}

/// The spawn of `op` a macro body holds, when it holds one.
///
/// A macro body arrives as unparsed tokens, and a token stream that holds
/// nothing but a fragment of an expression cannot be parsed as one. So the test
/// is looser than the one on a call: the identifier `Command` and a string
/// literal that names the `op` binary, anywhere in the same body. The looseness
/// costs an over-match nobody has hit and answers a spelling a stricter test
/// never sees.
fn spawn_inside_tokens(tokens: &TokenStream) -> Option<String> {
    let mut names_command = false;
    let mut program = None;
    read_tokens(tokens, &mut names_command, &mut program);

    match (names_command, program) {
        (true, Some(program)) => Some(format!("{COMMAND} … \"{program}\" in a macro body")),
        _ => None,
    }
}

/// Read every token of `tokens`, at any depth inside its groups.
///
/// A group is a bracketed run of tokens, and a macro body is full of them, so a
/// walk that reads only the top level reads almost nothing.
fn read_tokens(tokens: &TokenStream, names_command: &mut bool, program: &mut Option<String>) {
    for token in tokens.clone() {
        match token {
            TokenTree::Group(group) => read_tokens(&group.stream(), names_command, program),
            TokenTree::Ident(ident) => *names_command |= names_the_command_type(&ident),
            TokenTree::Literal(literal) => {
                if program.is_none() {
                    *program = op_binary_literal(&literal);
                }
            }
            TokenTree::Punct(_) => {}
        }
    }
}

/// Whether an identifier names the type whose constructor starts a process.
fn names_the_command_type(ident: &Ident) -> bool {
    ident == COMMAND
}

/// The program a literal names, when the literal is a string that names the
/// `op` binary.
fn op_binary_literal(literal: &Literal) -> Option<String> {
    let tokens = TokenStream::from(TokenTree::Literal(literal.clone()));
    let program = syn::parse2::<syn::LitStr>(tokens).ok()?.value();

    names_the_op_binary(&program).then_some(program)
}

/// Whether a program name names the 1Password CLI.
///
/// The last segment of a path decides, so `/opt/homebrew/bin/op` runs the same
/// binary as `op`, and `op-cache` runs neither.
fn names_the_op_binary(program: &str) -> bool {
    program
        .rsplit(PATH_SEPARATORS)
        .next()
        .is_some_and(|name| name == OP)
}
