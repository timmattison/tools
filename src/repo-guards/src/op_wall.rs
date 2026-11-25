//! Guard: only `op-cache` runs the `op` binary.
//!
//! Every secret this workspace reads lives in 1Password, and `op-cache` is the
//! one crate that reads it. It caches what it read, retries what failed, and
//! writes the cache file at mode 600. A crate that runs `op` itself gets none
//! of that, and the failure is quiet: the code works, the secret arrives, and
//! only the properties nobody can see are gone.
//!
//! `ufa` did exactly that. It ran `op item get ufa --vault Private --format
//! json` to learn which controllers the user keeps, and that command prints the
//! value of every concealed field beside its label. Every controller key in
//! that item reached a variable of `ufa` for a list of names. `op-cache` now
//! runs that command. The values still reach the process, in a buffer inside
//! `op-cache`, and stop there: no value reaches a type or a variable of `ufa`.
//! The reason the code reached around the helper is the one this file exists
//! to make loud: `op-cache` had no way to list the fields of an item, so the
//! helper was not usable for the job, and nothing said so.
//!
//! # The rule is one sentence
//!
//! A call of a spawn entrance whose arguments name the `op` binary, in any
//! crate but `op-cache`, is an offender. Locating `op` is not running it, so
//! `which::which("op")` is nobody's business here. Running `op-cache` itself is
//! the sanctioned route for a shell script, so `childenv::command("op-cache")`
//! is not an offender either.
//!
//! A spawn entrance is one of these callees:
//!
//! 1. **`childenv::command` and `childenv::fixture_command`.** `clippy.toml`
//!    bans every raw constructor of a process, so these two are the spelling
//!    that a new spawn of `op` arrives in. The last segment of the path
//!    decides, so the call behind an import, `command("op")`, is the same
//!    call. A method of the same name matches too.
//! 2. **`Command::new` and `CommandBuilder::new`.** The ban lets a site that
//!    states an exemption call them, and they started every spawn of this
//!    workspace before `childenv` existed. The last two segments of the path
//!    decide, so `std::process::Command::new`, `tokio::process::Command::new`
//!    and `portable_pty::CommandBuilder::new` all match.
//! 3. **A renamed import of one of those names.** `use childenv::command as
//!    spawn` makes `spawn` an entrance of the file, and `use
//!    std::process::Command as Process` makes `Process::new` one.
//!
//! The arguments name the `op` binary when they hold, at any depth, a string
//! literal whose last path segment is `op` or `op.exe`, in any letter case.
//! Thus `"/opt/homebrew/bin/op"` and `"OP"` run the same binary, and
//! `"op-cache"` runs a different one. A literal after the
//! closing parenthesis is an argument of a later method, so
//! `childenv::command("git").arg("op")` runs git.
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
//! matches, `op-cache` is audited like everything else, and its own spawn of
//! `op` is reported. The guard then fails loudly rather than quietly auditing
//! one crate fewer.
//!
//! # Parse to fail closed, then match over tokens
//!
//! "A call whose callee is a spawn entrance and whose arguments name `op`"
//! names a syntactic category, so a text search is the wrong tool. A search for
//! `Command::new("op")` answers only for the one spelling somebody thought of,
//! and every other spelling reports *clean*, which reads exactly like a guard
//! doing real work. The first version of this guard matched only a callee that
//! ended in `Command::new`. Then `clippy.toml` made `childenv::command` the one
//! entrance, and the guard reported each new spawn of `op` clean.
//!
//! Each file is parsed with [`syn::parse_file`] first. That is the fail-closed
//! half: a file this guard cannot read as Rust is a refusal, because "no spawn
//! here" and "I never understood this file" print identically.
//!
//! The verdict is then taken over the [`proc_macro2`] token stream of the file,
//! as [`git_env_sweep`](crate::git_env_sweep) does. [`syn`] leaves the body of
//! each macro invocation as unparsed tokens, so a walk of the syntax tree never
//! sees `println!("{:?}", childenv::command("op").output())`. The token stream
//! has no such blind spot, and one rule reads a macro body and a function body
//! the same way. The token stream also keeps **prose as data**. A `//` comment
//! never becomes a token, a doc comment becomes one string literal, and the
//! text of a string is one token, not the words it spells.
//!
//! The matched shape is an identifier followed immediately by a parenthesized
//! group: a call. The path in front of the identifier is the run of
//! `identifier ::` pairs before it. `command!(…)` puts a `!` between the two,
//! so a macro of that name is not a call.
//!
//! # The limits
//!
//! The matcher reads the callee and the arguments that the call site writes.
//! It does not see these spellings:
//!
//! - a program in a variable: `childenv::command(program)`.
//! - an entrance in a variable: `let spawn = childenv::command; spawn("op")`.
//! - a qualified path: `<Command>::new("op")`, whose tokens put `>` between
//!   the type and `::new`.
//! - a type alias of an entrance: `type Sh = std::process::Command;
//!   Sh::new("op")`. The walk learns a new name only from `X as Y`.
//!
//! To resolve a binding needs the name resolution of a compiler, which this
//! guard is not. A literal at the call, behind a plain path, is the shape of
//! every spawn in this workspace, and the shape of the one offender.
//!
//! The matcher over-matches on purpose. Each function or method named
//! `command` or `fixture_command` is an entrance, and so is each type named
//! `Command`, `clap::Command` included. A rename is an entrance in the whole
//! file, not only in the scope of its `use`. An over-match fails loudly, and a
//! person corrects it in an hour. An under-match stays green for years.
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
use std::str::FromStr;

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use thiserror::Error;

use crate::workspace_lints::{self, WorkspaceLintsError};

/// The crate that runs `op`, relative to the repository root.
const OP_CACHE: &str = "src/op-cache";

/// The name of the 1Password CLI, as a program name.
const OP: &str = "op";

/// The name of the 1Password CLI, as the file of a Windows install.
const OP_EXE: &str = "op.exe";

/// The functions that start a child process: the two constructors of
/// `childenv`.
const SPAWNING_FUNCTIONS: [&str; 2] = ["command", "fixture_command"];

/// The types whose constructor [`NEW`] starts a child process:
/// `std::process::Command`, `tokio::process::Command`, and
/// `portable_pty::CommandBuilder`.
const SPAWNING_TYPES: [&str; 2] = ["Command", "CommandBuilder"];

/// The constructor of each of [`SPAWNING_TYPES`].
const NEW: &str = "new";

/// The keyword that gives an import a new name.
const AS: &str = "as";

/// The character that `::` is two of, in a token stream.
const PATH_COLON: char = ':';

/// The separator of the segments of a rendered callee path.
const PATH_SEPARATOR: &str = "::";

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
        let unparsable = |message: String| OpWallError::Unparsable {
            path: path.clone(),
            message,
        };

        // The fail-closed half. `syn` returns an error rather than recovering
        // silently, so no verdict is taken from a partial read.
        syn::parse_file(&text).map_err(|error| unparsable(error.to_string()))?;
        let tokens = TokenStream::from_str(&text).map_err(|error| unparsable(error.to_string()))?;

        let spawns = spawns_of_op(tokens);
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

/// Every spawn of the `op` binary that the tokens of one file hold, sorted and
/// deduplicated.
///
/// The walk reads the file twice. The first pass learns the renamed imports of
/// the file, and the second pass reads each call. A `use` can come after the
/// call that it renames, so one pass is not sufficient.
fn spawns_of_op(tokens: TokenStream) -> BTreeSet<String> {
    let mut entrances = Entrances::new();
    entrances.learn_renames(tokens.clone());

    let mut found = BTreeSet::new();
    entrances.collect_spawns(tokens, &mut found);
    found
}

/// The names that a call of one file can start a process through.
///
/// The set starts as [`SPAWNING_FUNCTIONS`] and [`SPAWNING_TYPES`]. Each
/// `use … as` of one of those names adds its new name.
struct Entrances {
    /// The functions that start a process, the renamed ones included.
    functions: BTreeSet<String>,
    /// The types whose [`NEW`] starts a process, the renamed ones included.
    types: BTreeSet<String>,
}

impl Entrances {
    /// The entrances of a file that renames nothing.
    fn new() -> Self {
        Self {
            functions: SPAWNING_FUNCTIONS.into_iter().map(String::from).collect(),
            types: SPAWNING_TYPES.into_iter().map(String::from).collect(),
        }
    }

    /// Add the new name of each renamed import of an entrance in `tokens`, at
    /// any depth.
    ///
    /// The shape is three identifiers: an entrance, `as`, and the new name.
    /// `use childenv::{command as spawn, fixture_command}` puts that shape
    /// inside a brace group, so the walk follows every group.
    fn learn_renames(&mut self, tokens: TokenStream) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();

        for window in trees.windows(3) {
            let [TokenTree::Ident(original), TokenTree::Ident(keyword), TokenTree::Ident(renamed)] =
                window
            else {
                continue;
            };
            if keyword != AS {
                continue;
            }

            let original = original.to_string();
            if self.functions.contains(&original) {
                self.functions.insert(renamed.to_string());
            }
            if self.types.contains(&original) {
                self.types.insert(renamed.to_string());
            }
        }

        for tree in trees {
            if let TokenTree::Group(group) = tree {
                self.learn_renames(group.stream());
            }
        }
    }

    /// Record each spawn of `op` in `tokens`, at any depth.
    ///
    /// The recursion follows every group, so a call in a block, in a closure,
    /// in the arguments of another call, or in the body of a macro invocation
    /// is read the same way as a call at the top.
    fn collect_spawns(&self, tokens: TokenStream, found: &mut BTreeSet<String>) {
        let trees: Vec<TokenTree> = tokens.into_iter().collect();

        for (index, tree) in trees.iter().enumerate() {
            match tree {
                TokenTree::Ident(_) => {
                    // An identifier followed immediately by a parenthesized
                    // group is a call. `command!(…)` puts a `!` between the
                    // two, so a macro of that name is not read as one.
                    let Some(TokenTree::Group(arguments)) = trees.get(index.saturating_add(1))
                    else {
                        continue;
                    };
                    if arguments.delimiter() != Delimiter::Parenthesis {
                        continue;
                    }

                    let callee = callee_path(&trees, index);
                    if !self.is_entrance(&callee) {
                        continue;
                    }
                    if let Some(program) = op_program(arguments.stream()) {
                        found.insert(format!("{}(\"{program}\")", callee.join(PATH_SEPARATOR)));
                    }
                }
                TokenTree::Group(group) => self.collect_spawns(group.stream(), found),
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }

    /// Whether a callee path starts a process.
    ///
    /// A function decides by its last segment, so `childenv::command` and an
    /// imported `command` read the same. A constructor decides by its last two
    /// segments, so every module path in front of `Command::new` reads the
    /// same.
    fn is_entrance(&self, callee: &[String]) -> bool {
        match callee {
            [.., function] if self.functions.contains(function) => true,
            [.., spawning_type, constructor] => {
                constructor == NEW && self.types.contains(spawning_type)
            }
            _ => false,
        }
    }
}

/// The path of the callee whose last segment is the identifier at `index`.
///
/// The path is read back through each `identifier ::` pair in front of that
/// identifier, and it is given in source order: `childenv::command`, or
/// `std::process::Command::new`.
fn callee_path(trees: &[TokenTree], index: usize) -> Vec<String> {
    let mut segments = Vec::new();
    let mut position = index;

    while let Some(TokenTree::Ident(segment)) = trees.get(position) {
        segments.push(segment.to_string());

        let separated = position
            .checked_sub(2)
            .and_then(|start| trees.get(start..position))
            .is_some_and(is_path_separator);
        match position.checked_sub(3) {
            Some(previous) if separated => position = previous,
            _ => break,
        }
    }

    segments.reverse();
    segments
}

/// Whether two tokens are the `::` between two segments of a path.
fn is_path_separator(tokens: &[TokenTree]) -> bool {
    matches!(
        tokens,
        [TokenTree::Punct(first), TokenTree::Punct(second)]
            if first.as_char() == PATH_COLON && second.as_char() == PATH_COLON
    )
}

/// The first string literal in the arguments of a call, at any depth, that
/// names the `op` binary.
///
/// The whole of the arguments is read, not only the first token, so a literal
/// behind a reference or inside a nested call is found. That over-matches a
/// call that names `op` in an argument and runs something else, which is the
/// safe direction.
fn op_program(arguments: TokenStream) -> Option<String> {
    arguments.into_iter().find_map(|tree| match tree {
        TokenTree::Literal(literal) => match syn::Lit::new(literal) {
            syn::Lit::Str(text) => {
                Some(text.value()).filter(|program| names_the_op_binary(program))
            }
            _ => None,
        },
        TokenTree::Group(group) => op_program(group.stream()),
        TokenTree::Ident(_) | TokenTree::Punct(_) => None,
    })
}

/// Whether a program name names the 1Password CLI.
///
/// The last segment of a path decides, so `/opt/homebrew/bin/op` runs the same
/// binary as `op`, and `op-cache` runs neither. The segment is `op` or
/// `op.exe`, in any case. The default file system of macOS and the file system
/// of Windows ignore case, so `OP` finds the same binary.
fn names_the_op_binary(program: &str) -> bool {
    program
        .rsplit(PATH_SEPARATORS)
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case(OP) || name.eq_ignore_ascii_case(OP_EXE))
}
