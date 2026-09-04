//! The walk, and the way it tells a broken link from a working one.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use walkdir::WalkDir;

use crate::{line, Options, Summary};

/// What the operating system says about the target of one symbolic link.
#[derive(Debug)]
pub enum LinkState {
    /// The target resolves. The tool does nothing.
    Intact,
    /// The target is not there. This is the one state a repair acts on.
    Broken,
    /// The resolution failed, and not because the target is absent.
    ///
    /// A link with no read permission on a directory along its target, and a
    /// link that points at itself, both land here. The tool never rewrites such
    /// a link: it does not understand why the link failed, so it cannot know
    /// that a rewrite would be an improvement.
    Unresolvable(io::Error),
}

/// Asks the operating system to resolve the link at `path`.
///
/// `fs::metadata` follows the link, so its answer is about the target and not
/// about the link. Only [`io::ErrorKind::NotFound`] means the target is absent;
/// every other error means the question could not be answered at all, which is
/// a different thing and gets a different state.
pub fn classify(path: &Path) -> LinkState {
    match fs::metadata(path) {
        Ok(_) => LinkState::Intact,
        Err(error) if error.kind() == io::ErrorKind::NotFound => LinkState::Broken,
        Err(error) => LinkState::Unresolvable(error),
    }
}

/// Walks the tree under `options.root` and reports every broken link it finds.
///
/// `WalkDir` does not follow links, which is what this tool needs: a link is
/// the thing to examine, and a walk that followed one would examine the tree
/// behind it instead. The walk is not sorted, thus the order of the report
/// follows the order the directories come back in.
pub(crate) fn scan(
    options: &Options,
    out: &mut dyn Write,
    err: &mut dyn Write,
    summary: &mut Summary,
) {
    for entry in WalkDir::new(&options.root) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report_walk_error(err, &error);
                continue;
            }
        };

        // `file_type` here describes the entry itself, because the walk does
        // not follow links. A link that points at a directory is thus a link,
        // and not a directory to descend into.
        if !entry.file_type().is_symlink() {
            continue;
        }

        let path = entry.path();
        if options.verbose {
            line(err, format_args!("Found symlink: {}", path.display()));
        }

        match classify(path) {
            LinkState::Intact => {}
            // The `Error:` line and the count of this state belong to the next
            // slice, which adds them with tests of their own.
            LinkState::Unresolvable(_) => {}
            LinkState::Broken => report_broken(path, out, err, summary),
        }
    }
}

/// Reports one broken link and counts it.
///
/// The target is read only now, after the link is known to be broken, so a tree
/// of working links costs one system call for each link and no more.
fn report_broken(path: &Path, out: &mut dyn Write, err: &mut dyn Write, summary: &mut Summary) {
    let target = match fs::read_link(path) {
        Ok(target) => target,
        Err(error) => {
            line(
                err,
                format_args!(
                    "Warning: cannot read the symlink {}: {error}",
                    path.display()
                ),
            );
            return;
        }
    };

    line(
        out,
        format_args!("Broken symlink: {} -> {}", path.display(), target.display()),
    );
    summary.broken += 1;
}

/// Reports one error the walk raised, and lets the walk go on.
///
/// `walkdir` does not always know which path an error belongs to: an error the
/// iterator raises about itself, such as a depth limit, carries none. There is
/// nothing to name in that line, so it drops the path and the colon and writes
/// the error alone. The `Warning: ` prefix stays either way, because that is
/// what a reader greps for.
fn report_walk_error(err: &mut dyn Write, error: &walkdir::Error) {
    match error.path() {
        Some(path) => line(
            err,
            format_args!("Warning: cannot read {}: {error}", path.display()),
        ),
        None => line(err, format_args!("Warning: {error}")),
    }
}
