//! The walk, and the way it tells a broken link from a working one.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::{line, repair, Options, Summary};

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

        // This match is the whole gate a repair sits behind. A repair belongs
        // in `report_broken` and nowhere else, so a link the tool could not
        // resolve reaches no repair at all — not by a check inside the repair,
        // which a later change could forget, but because the two states never
        // meet the same code.
        match classify(path) {
            LinkState::Intact => {}
            LinkState::Unresolvable(error) => report_unresolvable(path, &error, err, summary),
            LinkState::Broken => report_broken(options, path, out, err, summary),
        }
    }
}

/// Reads the target of the link at `path`, and writes the warning line when
/// that read fails.
///
/// A link is read only once the tool has something to say about it, so a tree
/// of working links costs one system call for each link and no more.
///
/// A link whose target cannot be read counts in nothing at all. Both of the
/// lines this module writes name the target, neither one can name a target it
/// could not read, and a count with no line under it is a number a reader
/// cannot act on.
fn read_target(path: &Path, err: &mut dyn Write) -> Option<PathBuf> {
    match fs::read_link(path) {
        Ok(target) => Some(target),
        Err(error) => {
            line(
                err,
                format_args!(
                    "Warning: cannot read the symlink {}: {error}",
                    path.display()
                ),
            );
            None
        }
    }
}

/// Reports one broken link, counts it, and repairs it when the options give
/// the tool a way to.
///
/// This is where the repair sits. The link that reaches this function is the
/// one link the operating system says is absent, which is the one state a
/// rewrite can improve, and its target is already in hand.
///
/// The report names the broken link before the repair runs, thus a reader sees
/// every link the tool found even when a repair fails, and a run that repairs
/// everything still says what was wrong.
///
/// A repair that fails writes its warning to the error stream and counts
/// nothing. The link is still broken, the report already says so, and a count
/// of repairs that included the ones that did not happen would be a number a
/// reader cannot act on.
///
/// A run under [`Options::dry_run`] plans the repair, reports the plan, counts
/// it in [`Summary::fixed`], and leaves the link alone.
fn report_broken(
    options: &Options,
    path: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
    summary: &mut Summary,
) {
    let Some(target) = read_target(path, err) else {
        return;
    };

    line(
        out,
        format_args!("Broken symlink: {} -> {}", path.display(), target.display()),
    );
    summary.broken += 1;

    let Some(plan) = repair::plan(options, path, target.as_os_str(), err) else {
        return;
    };

    // A dry run stops here, and this is the whole of the flag. Everything above
    // this point only read the tree: `repair::plan` builds candidates and asks
    // the operating system whether each one resolves, and it writes nothing at
    // all. `repair::apply` is the one call that changes the tree, thus it is the
    // one call a dry run leaves out, and the plan a dry run reports is the plan
    // a real run over the same tree would carry out.
    if options.dry_run {
        report_repair(out, "Would fix", path, &plan);
        summary.fixed += 1;
        return;
    }

    match repair::apply(path, &plan.target) {
        Ok(()) => {
            report_repair(out, "Fixed", path, &plan);
            summary.fixed += 1;
        }
        Err(error) => line(
            err,
            format_args!("Warning: cannot replace {}: {error}", path.display()),
        ),
    }
}

/// Writes the line that names one repair, which `opening` says was made or was
/// only planned.
///
/// The report says `Fixed symlink by {phrase}: {link} -> {target}` for a repair
/// that happened and `Would fix symlink by {phrase}: ...` for one a dry run
/// planned. The two lines differ in their first words and in nothing else, so
/// they are one line here: a change to what a repair line names reaches both
/// wordings, and neither one can drift away from the other.
fn report_repair(out: &mut dyn Write, opening: &str, link: &Path, plan: &repair::Repair) {
    line(
        out,
        format_args!(
            "{opening} symlink by {}: {} -> {}",
            plan.strategy.phrase(),
            link.display(),
            Path::new(&plan.target).display()
        ),
    );
}

/// Reports one link the operating system refused to resolve, and counts it.
///
/// The line goes to the error stream, because it is a diagnostic and not a
/// finding: the tool asked a question about the target and got no answer, so it
/// knows nothing about that target and says so.
///
/// This never touches [`Summary::broken`], thus the closing summary of a run
/// with such links and no broken ones is still `No broken symlinks found.`.
/// That is deliberate, and the reason is the tool this port replaces: its
/// summary counts broken links, its diagnostics go to the error stream, and it
/// gives status 0 whatever it wrote there. A second summary line would be a new
/// number in the report of a tool whose report other things already read.
fn report_unresolvable(path: &Path, error: &io::Error, err: &mut dyn Write, summary: &mut Summary) {
    let Some(target) = read_target(path, err) else {
        return;
    };

    line(
        err,
        format_args!(
            "Error: cannot resolve {} -> {}: {error}",
            path.display(),
            target.display()
        ),
    );
    summary.errors += 1;
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
