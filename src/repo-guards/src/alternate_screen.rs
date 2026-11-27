//! Guard: no source file in this workspace enters the alternate screen by itself.
//!
//! A full-screen tool enters the alternate screen so that the shell screen comes
//! back when the tool ends. The mosh server ignores the mode that does this,
//! mode 1049, so on a mosh session the tool draws over the shell text. The tool
//! must erase the screen in the same write. `ratatui` sends no erase at
//! startup, and two tools of this workspace shipped without one.
//!
//! The crate `altscreen` holds the one entrance. `altscreen::Session::enter`
//! and `altscreen::RatatuiSession::enter` (feature `ratatui`) erase the screen
//! in the same write and restore the terminal on every exit. This guard refuses
//! every other way to enter the alternate screen.
//!
//! # Why clippy cannot say all of it
//!
//! `clippy.toml` bans what clippy can see: the `use` spellings, and the
//! `ratatui` and `termion` functions. Clippy has three gaps.
//!
//! - `disallowed-types` checks `use` items. A unit struct in value position,
//!   such as the entry command written as a path in a macro argument, has no
//!   `use` item, so the ban stays quiet.
//! - `disallowed-methods` refuses a unit struct, because a unit struct is not a
//!   method.
//! - Clippy reads no string literal, so a raw `ESC [ ? 1049 h` in a `print!`
//!   passes.
//!
//! This guard covers those three gaps.
//!
//! # The two rules
//!
//! [`audit`] judges two shapes, wherever they appear.
//!
//! 1. **An identifier.** A token that is an identifier equal to one of
//!    [`IDENTIFIERS`], with a leading `r#` removed. The match is exact, so a
//!    name that only contains one of them is not a match. A `use … as` rename is
//!    caught, because the original name is an identifier token of the `use`
//!    item.
//! 2. **A string literal that holds an entry sequence.** The guard takes the
//!    value of the literal, after escapes. A byte string and a C string give
//!    their bytes, and each byte stands for the character of the same number.
//!    The forms `\x1b`, `\x1B`, `\u{1b}`, a raw ESC character, and the 8-bit
//!    introducer U+009B thus reduce to one form. The guard looks for an
//!    introducer (ESC then `[`, or U+009B), then `?`, then the parameters
//!    (digits and `;`), then a final character. If the final character is `h`,
//!    each parameter in [`MODES`] is a violation. The recorded spelling is
//!    `CSI ?<mode> h`. A leave sequence ends in `l` and is not recorded. A char
//!    literal and a number are not read.
//!
//! # Why tokens
//!
//! `syn` leaves the body of a macro invocation unparsed, and an entry command
//! is nearly always an argument of `execute!` or `write!`. A tree walk cannot
//! see it. Tokens keep what the tree gives: a `//` comment never becomes a
//! token, and a doc comment becomes one string literal, as does each string. So
//! prose that names a command stays data. This documentation writes the
//! escapes as text, and the value of that text holds a backslash, not an ESC,
//! so the guard does not flag its own documentation. Each file is parsed first,
//! so a file that the guard cannot read as Rust is a refusal, not a clean
//! verdict.
//!
//! # Over-match
//!
//! A local type named `AlternateScreen` is reported. That failure is loud, and
//! a rename fixes it. An over-matching guard gets repaired. An under-matching
//! guard reports clean and stays that way.
//!
//! # What the guard does not prove
//!
//! - **A sequence built at runtime.** `format!("\x1b[?{}h", 1049)` writes no
//!   complete sequence into one literal.
//! - **A sequence that `concat!` joins from parts.** Each part is a literal on
//!   its own, and none holds the whole sequence.
//! - **`ratatui::init`.** Clippy catches that function.
//!
//! # Exemptions
//!
//! [`EXEMPTIONS`] is keyed on the file and the spelling, so an exemption for one
//! spelling does not exempt the file that holds it. A new spelling in an exempt
//! file is still a violation. An entry that matches nothing is reported by
//! [`Report::unused_exemptions`], because an allowlist that nobody prunes stops
//! covering what it was written for.
//!
//! # The read set
//!
//! The files are [`crate::git_env_sweep::source_files`]: the Rust that cargo
//! compiles. The companion test of that guard measures the set against
//! `cargo metadata`, so this guard does not repeat the measurement.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::{TokenStream, TokenTree};
use thiserror::Error;

use crate::git_env_sweep::{self, GitEnvSweepError};

/// The identifiers that name an entry into the alternate screen: the entry
/// command of `crossterm`, the command and the wrapper type of `termion`, the
/// extension trait of `termion`, and its method.
pub const IDENTIFIERS: [&str; 5] = [
    "EnterAlternateScreen",
    "ToAlternateScreen",
    "AlternateScreen",
    "IntoAlternateScreen",
    "into_alternate_screen",
];

/// The DEC private modes that switch to the alternate screen.
pub const MODES: [u32; 3] = [47, 1047, 1049];

/// One deliberate use of an entry spelling.
///
/// Keyed on the spelling as well as the file, so exempting one spelling does
/// not exempt the file that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exemption {
    /// The file, relative to the repository root, spelled with `/` separators.
    pub file: &'static str,
    /// The spelling that file holds, for example `EnterAlternateScreen`.
    pub spelling: &'static str,
    /// Why the use is deliberate.
    pub reason: &'static str,
}

/// Every use of an entry spelling that is deliberate.
///
/// Each entry is final. Two cover `altscreen` itself, the one entrance, and one
/// covers the pty tests of `krt`.
///
/// Public so a test can measure the list against what the repository holds. An
/// entry that matches nothing is reported by [`Report::unused_exemptions`].
pub const EXEMPTIONS: [Exemption; 5] = [
    Exemption {
        file: "src/altscreen/src/lib.rs",
        spelling: "EnterAlternateScreen",
        reason: "the one entrance; it renders the entry command into the bytes it writes",
    },
    Exemption {
        file: "src/altscreen/src/lib.rs",
        spelling: "CSI ?1049 h",
        reason: "its unit tests assert the exact entry bytes",
    },
    Exemption {
        file: "src/krt/tests/terminal.rs",
        spelling: "CSI ?1049 h",
        reason: "the pty tests search the output of `krt` for the entry sequence; \
                 they write to no user terminal",
    },
    Exemption {
        file: "src/termstate/src/tests.rs",
        spelling: "CSI ?1049 h",
        reason: "the tests feed the entry sequence to the emulator as data; \
                 they write to no user terminal",
    },
    Exemption {
        file: "src/nle/src/filters/keystroke_hud/tests.rs",
        spelling: "CSI ?1049 h",
        reason: "a test feeds the entry sequence of a full-screen program to the \
                 HUD as data; it writes to no user terminal",
    },
];

/// Everything that can stop the audit from reaching a verdict.
///
/// Every variant is a refusal. A guard that cannot read the workspace says so,
/// because "nothing enters the alternate screen" and "nothing was read" look the
/// same in a CI log.
#[derive(Debug, Error)]
pub enum AlternateScreenError {
    /// The set of source files could not be read.
    #[error("cannot read the set of source files: {0}")]
    ReadSet(#[from] GitEnvSweepError),

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
}

/// One file that enters the alternate screen by itself.
#[derive(Debug, Clone)]
pub struct Offender {
    /// The file, relative to the repository root.
    path: PathBuf,
    /// The spellings it holds, sorted and deduplicated.
    spellings: Vec<String>,
}

impl Offender {
    /// The offending file, relative to the repository root.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The spellings this file holds, sorted and deduplicated.
    #[must_use]
    pub fn spellings(&self) -> &[String] {
        &self.spellings
    }
}

/// The verdict of one audit: what was read, and which files enter the
/// alternate screen by themselves.
#[derive(Debug, Clone)]
pub struct Report {
    /// Every file read, relative to the repository root and sorted.
    files: Vec<PathBuf>,
    /// Offending files, sorted by path.
    offenders: Vec<Offender>,
    /// The exemptions that matched a spelling present in the source.
    applied: BTreeSet<(String, String)>,
}

impl Report {
    /// True when no file enters the alternate screen by itself.
    ///
    /// An unused exemption is not a compliance failure. It is a fact about the
    /// allowlist, so [`unused_exemptions`](Self::unused_exemptions) reports it.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.offenders.is_empty()
    }

    /// The offending files, sorted by path.
    #[must_use]
    pub fn offenders(&self) -> &[Offender] {
        &self.offenders
    }

    /// Every file the audit read, relative to the repository root and sorted.
    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// How many files the audit read. A caller asserts that this is not zero.
    #[must_use]
    pub fn files_examined(&self) -> usize {
        self.files.len()
    }

    /// The entries of [`EXEMPTIONS`] that matched no spelling in the source.
    #[must_use]
    pub fn unused_exemptions(&self) -> Vec<&'static Exemption> {
        EXEMPTIONS
            .iter()
            .filter(|exemption| {
                !self
                    .applied
                    .contains(&(exemption.file.to_owned(), exemption.spelling.to_owned()))
            })
            .collect()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offenders.is_empty() {
            return write!(
                f,
                "Read {} Rust source files; none enters the alternate screen by itself.",
                self.files.len()
            );
        }

        writeln!(
            f,
            "{} of {} Rust source files enter the alternate screen by themselves.",
            self.offenders.len(),
            self.files.len()
        )?;
        writeln!(
            f,
            "The mosh server ignores mode 1049, and ratatui sends no erase at startup, so a tool \
             that enters by itself draws over the shell text."
        )?;
        writeln!(
            f,
            "Enter through `altscreen::Session::enter`, or `altscreen::RatatuiSession::enter` \
             (feature `ratatui`). Both erase the screen in the same write and restore the \
             terminal on every exit."
        )?;

        for offender in &self.offenders {
            writeln!(f)?;
            writeln!(
                f,
                "{} holds: {}",
                offender.path.display(),
                offender.spellings.join(", ")
            )?;
        }

        Ok(())
    }
}

/// Read every Rust source file of the workspace at `repo_root` and report the
/// ones that enter the alternate screen by themselves.
///
/// # Errors
///
/// Returns [`AlternateScreenError`] when the set of files cannot be read, or a
/// file cannot be read or parsed.
pub fn audit(repo_root: &Path) -> Result<Report, AlternateScreenError> {
    let files = git_env_sweep::source_files(repo_root)?;

    let mut offenders = Vec::new();
    let mut applied = BTreeSet::new();

    for relative in &files {
        let found = spellings(&repo_root.join(relative))?;
        if found.is_empty() {
            continue;
        }

        let key = relative.to_string_lossy().replace('\\', "/");
        let mut remaining = Vec::new();
        for spelling in found {
            if EXEMPTIONS
                .iter()
                .any(|exemption| exemption.file == key && exemption.spelling == spelling)
            {
                applied.insert((key.clone(), spelling));
                continue;
            }
            remaining.push(spelling);
        }

        if !remaining.is_empty() {
            offenders.push(Offender {
                path: relative.clone(),
                spellings: remaining,
            });
        }
    }

    Ok(Report {
        files,
        offenders,
        applied,
    })
}

/// The entry spellings `path` holds, sorted and deduplicated.
fn spellings(path: &Path) -> Result<Vec<String>, AlternateScreenError> {
    let source = fs::read_to_string(path).map_err(|source| AlternateScreenError::ReadSource {
        path: path.to_path_buf(),
        source,
    })?;

    // The fail-closed half: a file that is not valid Rust is refused.
    syn::parse_file(&source).map_err(|source| AlternateScreenError::ParseSource {
        path: path.to_path_buf(),
        source,
    })?;

    let tokens =
        TokenStream::from_str(&source).map_err(|error| AlternateScreenError::ParseSource {
            path: path.to_path_buf(),
            source: syn::Error::new(proc_macro2::Span::call_site(), error),
        })?;

    let mut found = BTreeSet::new();
    collect(tokens, &mut found);
    Ok(found.into_iter().collect())
}

/// The escape character that opens a 7-bit control sequence introducer.
const ESCAPE: char = '\u{1b}';

/// The single-character 8-bit control sequence introducer.
const CSI_8_BIT: char = '\u{9b}';

/// Walk `tokens`, recording every entry spelling to any depth.
///
/// Recursion follows every group, so a spelling inside the body of a macro
/// invocation is reached the same way a top-level one is.
fn collect(tokens: TokenStream, found: &mut BTreeSet<String>) {
    for tree in tokens {
        match tree {
            TokenTree::Ident(ident) => {
                let text = ident.to_string();
                let name = text.strip_prefix("r#").unwrap_or(&text);
                if IDENTIFIERS.contains(&name) {
                    found.insert(name.to_owned());
                }
            }
            TokenTree::Literal(literal) => {
                if let Some(value) = literal_value(literal) {
                    collect_modes(&value, found);
                }
            }
            TokenTree::Group(group) => collect(group.stream(), found),
            TokenTree::Punct(_) => {}
        }
    }
}

/// The value of a string, byte string, or C string literal, after escapes, as
/// characters. Each byte of a byte string or a C string stands for the
/// character of the same number. Any other literal gives `None`.
fn literal_value(literal: proc_macro2::Literal) -> Option<Vec<char>> {
    match syn::Lit::new(literal) {
        syn::Lit::Str(text) => Some(text.value().chars().collect()),
        syn::Lit::ByteStr(bytes) => Some(bytes.value().into_iter().map(char::from).collect()),
        syn::Lit::CStr(bytes) => Some(
            bytes
                .value()
                .to_bytes()
                .iter()
                .copied()
                .map(char::from)
                .collect(),
        ),
        _ => None,
    }
}

/// Record every alternate-screen entry sequence that `value` holds.
///
/// A sequence is an introducer, `?`, parameters of digits and `;`, and a final
/// `h`. Each parameter in [`MODES`] is recorded as `CSI ?<mode> h`.
fn collect_modes(value: &[char], found: &mut BTreeSet<String>) {
    for (index, &character) in value.iter().enumerate() {
        let after_introducer = match character {
            ESCAPE if value.get(index + 1) == Some(&'[') => index + 2,
            CSI_8_BIT => index + 1,
            _ => continue,
        };
        if value.get(after_introducer) != Some(&'?') {
            continue;
        }

        let parameters = &value[after_introducer + 1..];
        let length = parameters
            .iter()
            .take_while(|c| c.is_ascii_digit() || **c == ';')
            .count();
        if parameters.get(length) != Some(&'h') {
            continue;
        }

        let text: String = parameters[..length].iter().collect();
        for mode in text.split(';').filter_map(|part| part.parse::<u32>().ok()) {
            if MODES.contains(&mode) {
                found.insert(format!("CSI ?{mode} h"));
            }
        }
    }
}
