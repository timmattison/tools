//! The repair: which new target to try, whether the operating system will
//! resolve it, and how the new link takes the place of the old one.
//!
//! [`plan`] builds candidates and accepts the first one that resolves, and
//! [`apply`] writes the accepted candidate into the link. The two are apart
//! because they answer different questions: one reads the tree and decides,
//! the other writes to the tree and can fail on its own.
//!
//! The state of a repair is the [`Option`] that [`plan`] gives back, and there
//! is no flag beside it. The tool this port replaces carries a `fixed` boolean
//! that its prepend branch sets and its remove branch never does, so its remove
//! branch runs after a repair that already happened. Here the strategies are
//! one chain of `or_else`: the first candidate that resolves ends the chain,
//! and `None` means no strategy had one. Nothing can fall out of step with a
//! flag that does not exist.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{line, pathbytes, Options};

/// How many names the replacement tries before it gives the collision back.
///
/// A name carries the process id, a nanosecond stamp and a counter, so a
/// collision needs two processes to land on the same nanosecond. A few names
/// are thus enough, and a bounded count keeps a directory that answers
/// `AlreadyExists` for some other reason from becoming an endless loop.
const NAME_ATTEMPTS: usize = 16;

/// Which strategy built a new target. It decides the message the run prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The new target is the old one with a string in front of it.
    Prepend,
    /// The new target is the old one with a prefix taken off the front.
    Remove,
}

impl Strategy {
    /// The phrase that names this strategy in the report.
    ///
    /// The report says `Fixed symlink by {phrase}` for a repair that happened,
    /// so the wording of a strategy lives with the strategy and not at the line
    /// that prints it.
    #[must_use]
    pub fn phrase(self) -> &'static str {
        match self {
            Self::Prepend => "prepending",
            Self::Remove => "removing prefix",
        }
    }
}

/// A repair the tool can make to one broken link.
#[derive(Debug, Clone)]
pub struct Repair {
    /// The exact bytes that go into the new link.
    pub target: OsString,
    /// The strategy that built [`Repair::target`].
    pub strategy: Strategy,
}

/// Builds the first candidate target that resolves, or gives back `None`.
///
/// `link` is the path of the broken symbolic link itself, and `target` is what
/// that link holds today. The strategies are tried in the order of this chain
/// and the first one whose candidate resolves ends it, so the order a reader
/// wants to know is one expression and a new strategy joins it by adding one
/// link.
///
/// There is no flag beside this chain that says whether a repair was found.
/// The tool this port replaces carries a `fixed` boolean, its prepend branch
/// sets it, its remove branch never does, and nothing reads the flag after that
/// point — so the omission changes nothing today, which is exactly why it
/// survived. Here the [`Option`] this function gives back **is** that state:
/// `Some` ends the chain and `None` says no strategy had a candidate that
/// resolved. The order of the strategies is thus one expression a reader sees
/// whole, and there is no second place for the order and the state to disagree.
///
/// A link with no parent directory gets no repair. Such a link cannot come out
/// of a walk of a directory — every entry a walk finds sits in the directory it
/// was found in — and a name with no directory to resolve against is a question
/// this function cannot answer, so it answers `None`.
///
/// Each strategy refuses a candidate with no bytes in it, and it does so on its
/// own. A guard on the result of this chain would read the same way and behave
/// differently: an empty candidate from the prepend would end the chain there,
/// and the remove would never get its turn on a link it can repair. A guard
/// inside a strategy is a `None` like any other, so the chain falls through.
pub fn plan(options: &Options, link: &Path, target: &OsStr, err: &mut dyn Write) -> Option<Repair> {
    let link_dir = link.parent()?;

    prepend(options, link, link_dir, target, err)
        .or_else(|| remove(options, link, link_dir, target, err))
}

/// Puts `options.prepend` in front of `target`, and accepts the result when the
/// link would resolve to a file that is there.
///
/// A candidate with no bytes in it is refused before the check, as it is in the
/// remove. This half needs an empty prefix and an empty target together, and
/// the operating system refuses to make a link with an empty target, so nothing
/// is known to reach it. It is here because the rule belongs to the check and
/// not to one strategy: the reason the check cannot answer for an empty
/// candidate is the same on both sides, and a rule that holds in one place and
/// is a claim about link targets in the other is a rule that a later change
/// breaks in silence.
fn prepend(
    options: &Options,
    link: &Path,
    link_dir: &Path,
    target: &OsStr,
    err: &mut dyn Write,
) -> Option<Repair> {
    let prefix = options.prepend.as_ref()?;

    // `OsString::push` takes any `OsStr` on every platform, so building the
    // candidate needs no platform code and no indexing of a string.
    let mut candidate = OsString::from(prefix);
    candidate.push(target);

    if options.verbose {
        line(
            err,
            format_args!(
                "Attempting to fix by prepending: {}: {} -> {}",
                link.display(),
                Path::new(target).display(),
                Path::new(&candidate).display()
            ),
        );
    }

    if candidate.is_empty() {
        if options.verbose {
            line(
                err,
                format_args!("Prepended target is empty: {}", link.display()),
            );
        }
        return None;
    }

    if fs::metadata(resolved(link_dir, &candidate)).is_ok() {
        return Some(Repair {
            target: candidate,
            strategy: Strategy::Prepend,
        });
    }

    if options.verbose {
        line(
            err,
            format_args!(
                "Prepended target does not exist: {} -> {}",
                link.display(),
                Path::new(&candidate).display()
            ),
        );
    }
    None
}

/// Takes `options.remove` off the front of `target`, and accepts the result
/// when the link would resolve to a file that is there.
///
/// A target that does not start with the prefix gives `None` before anything is
/// written or read, so a run with `--remove-to-fix` costs nothing on the links
/// it does not describe.
///
/// A prefix that is the whole target leaves no bytes at all, and such a
/// candidate is refused before the check. The check cannot answer for it: the
/// directory that holds the link, joined with nothing, is that directory again,
/// and the directory is there, so the answer is about the directory and never
/// about a target. A run that took that answer wrote an empty target over the
/// link, and the text the link held is then gone from the only place that held
/// it. `--remove-to-fix /old/path/` reaches this on any link whose target is
/// exactly `/old/path/`.
fn remove(
    options: &Options,
    link: &Path,
    link_dir: &Path,
    target: &OsStr,
    err: &mut dyn Write,
) -> Option<Repair> {
    let prefix = options.remove.as_ref()?;

    // A raw byte prefix, as the tool this port replaces has. `pathbytes` says
    // there why `Path::strip_prefix` would answer a different question.
    let candidate = pathbytes::strip_prefix(target, prefix)?;

    if options.verbose {
        line(
            err,
            format_args!(
                "Attempting to fix by removing prefix: {}: {} -> {}",
                link.display(),
                Path::new(target).display(),
                Path::new(&candidate).display()
            ),
        );
    }

    if candidate.is_empty() {
        if options.verbose {
            line(
                err,
                format_args!("Target with removed prefix is empty: {}", link.display()),
            );
        }
        return None;
    }

    if fs::metadata(resolved(link_dir, &candidate)).is_ok() {
        return Some(Repair {
            target: candidate,
            strategy: Strategy::Remove,
        });
    }

    if options.verbose {
        line(
            err,
            format_args!(
                "Target with removed prefix does not exist: {} -> {}",
                link.display(),
                Path::new(&candidate).display()
            ),
        );
    }
    None
}

/// The path the operating system will resolve the new link to.
///
/// The Go tool writes the unresolved candidate into the new link and checks
/// `filepath.Join(dir, candidate)`. `filepath.Join` **appends** an absolute
/// second argument rather than replacing the first, so the two spellings agree
/// only while the candidate stays relative. With an absolute candidate the Go
/// tool checks one path, writes another, prints `Fixed symlink by prepending:`,
/// and leaves a different broken link in place of the old one.
///
/// `Path::join` **replaces** the first path when the second is absolute, which
/// is exactly how the operating system resolves a symlink: a relative target
/// resolves against the directory that holds the link, and an absolute target
/// resolves against the root. So this function names the path the link will
/// resolve to, and not a second spelling of it. The check and the write thus
/// name the same file.
///
/// That holds for every target but one. A target with no bytes in it names no
/// file at all, while `link_dir.join("")` gives `link_dir` back, so this
/// function would answer about the directory that holds the link. Both
/// strategies refuse an empty candidate before they reach this call, which is
/// what keeps the sentence above true of everything that arrives here.
fn resolved(link_dir: &Path, target: &OsStr) -> PathBuf {
    link_dir.join(target)
}

/// Replaces the symlink at `link` so that it holds `target`, in one step.
///
/// The new link is made under a name of its own in the directory that holds
/// `link`, and then renamed over `link`. `rename` over a symlink is atomic on
/// macOS and on Linux, so a process that dies during the repair leaves either
/// the old link or the new one, and never no link at all. The Go tool calls
/// `os.Remove` and then `os.Symlink`, and a death between those two leaves the
/// path empty.
///
/// That guarantee is not visible to a black box test in one thread: both
/// designs end with the same link on the disk. So the tests pin what is
/// visible — a repair leaves no temporary entry behind, on the path where the
/// rename works and on the path where it fails.
///
/// # Errors
///
/// Gives back the error the operating system raised while it made the new link
/// or while that link took the place of the old one. A failed rename takes the
/// temporary link away again, so a repair that fails leaves nothing behind.
pub fn apply(link: &Path, target: &OsStr) -> io::Result<()> {
    // A link that a walk found always sits in a directory. A bare name has an
    // empty parent, and joining onto an empty path gives the bare name back, so
    // the temporary link is a sibling of the old one either way.
    let link_dir = link.parent().unwrap_or_else(|| Path::new(""));

    for _ in 0..NAME_ATTEMPTS {
        let temporary = link_dir.join(temporary_name());
        match create_symlink(target, &temporary) {
            Ok(()) => {
                return match fs::rename(&temporary, link) {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // `remove_file` takes a symlink away without following
                        // it, thus this removes the link that was just made and
                        // never the file it names.
                        let _ = fs::remove_file(&temporary);
                        Err(error)
                    }
                };
            }
            // Another repair, in this process or in another one, holds this
            // name. Take the next one.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "no free temporary name in {} after {NAME_ATTEMPTS} tries",
            link_dir.display()
        ),
    ))
}

/// A name for the new link, before it takes the place of the old one.
///
/// The name carries the process id, the current nanosecond and a counter that
/// belongs to the whole process, so two threads of one process, two processes
/// on one machine, and one thread that repairs two links inside one nanosecond
/// all get names of their own. The leading dot keeps the name out of a listing
/// that hides dot files, for the moment it exists.
fn temporary_name() -> OsString {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    // A clock that answers with a time before the epoch says nothing useful
    // about uniqueness, and the counter carries that on its own.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);

    OsString::from(format!(".symfix-{}-{nanos}-{counter}", std::process::id()))
}

/// Makes a symbolic link at `link` that holds `target`.
#[cfg(unix)]
fn create_symlink(target: &OsStr, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows needs the kind of the target at the moment the link is made, and it
/// needs a privilege the user does not hold by default. This port does not
/// guess at either one.
#[cfg(not(unix))]
fn create_symlink(target: &OsStr, link: &Path) -> io::Result<()> {
    let _ = (target, link);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "symfix cannot create symlinks on this platform",
    ))
}
