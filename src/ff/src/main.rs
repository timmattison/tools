use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

#[cfg(target_os = "macos")]
use anyhow::Context;
use buildinfo::version_string;
use clap::Parser;
use ff::fs::RealFs;
use ff::pattern::{Case, Matcher};
use ff::plan::{named_root, Mount, SkippedMount};
use ff::summary::{Summary, Volumes};
use ff::walk::{walk, Options, WalkRoot};

/// The exit status of a run that read each subtree to its end.
const EXIT_COMPLETE: u8 = 0;
/// The exit status of a run that could not read one or more subtrees.
const EXIT_INCOMPLETE: u8 = 1;
/// The exit status of a run that could not start, or could not write a match.
const EXIT_ERROR: u8 = 2;

/// Finds files by name across a whole machine. An interrupted system call is
/// done again, thus the walk never ends early.
#[derive(Parser)]
#[command(name = "ff", version = version_string!())]
struct Cli {
    /// The glob that each file name must match, for example '120.mp4' or
    /// '*.MP4'. It ignores case unless -s is given.
    pattern: String,

    /// The directories to search. With none, ff searches each mounted volume
    /// that holds user files (macOS only).
    paths: Vec<PathBuf>,

    /// Match the case of each letter.
    #[arg(short = 's', long)]
    case_sensitive: bool,

    /// Take a regular expression instead of a glob. It matches anywhere in the
    /// name.
    #[arg(short = 'e', long)]
    regex: bool,

    /// Stop at each mount point.
    #[arg(short = 'x', long)]
    one_file_system: bool,

    /// Also search the File Provider directories: each directory in
    /// ~/Library/CloudStorage, and ~/Library/Mobile Documents.
    #[arg(long)]
    include_cloud: bool,

    /// Name each subtree that the search could not read, and each directory
    /// and volume that it skipped.
    #[arg(long)]
    show_skipped: bool,

    /// End each match with a NUL instead of a line end, for xargs -0.
    #[arg(short = '0', long)]
    print0: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("ff: {error:#}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

/// Searches, writes each match and the summary, and gives the exit status.
fn run(cli: &Cli) -> anyhow::Result<u8> {
    let case = if cli.case_sensitive {
        Case::Sensitive
    } else {
        Case::Insensitive
    };
    let matcher = if cli.regex {
        Matcher::regex(&cli.pattern, case)?
    } else {
        Matcher::glob(&cli.pattern, case)?
    };
    let search = plan(cli)?;
    let terminator: &[u8] = if cli.print0 { b"\0" } else { b"\n" };
    let stdout = io::stdout();

    let started = Instant::now();
    let report = walk(
        &RealFs,
        &search.roots,
        Options {
            include_cloud: cli.include_cloud,
        },
        &matcher,
        |path: &Path| {
            let mut out = stdout.lock();
            out.write_all(path.as_os_str().as_bytes())?;
            out.write_all(terminator)?;
            out.flush()
        },
    );
    let elapsed = started.elapsed();

    if let Some(error) = &report.output_error {
        // The reader stopped, for example `head`. It has each match that it
        // wanted, and the walk has nobody to tell more to.
        if error.kind() == io::ErrorKind::BrokenPipe {
            return Ok(EXIT_COMPLETE);
        }
        anyhow::bail!("cannot write a match: {error}");
    }

    let summary = Summary {
        report: &report,
        volumes: search.skipped_volumes.as_deref().map(|skipped| Volumes {
            searched: search.roots.len(),
            skipped,
        }),
        elapsed,
        show_skipped: cli.show_skipped,
    };
    let mut stderr = io::stderr().lock();
    for line in summary.lines() {
        writeln!(stderr, "{line}")?;
    }
    Ok(if report.is_complete() {
        EXIT_COMPLETE
    } else {
        EXIT_INCOMPLETE
    })
}

/// The trees to walk.
struct Search {
    roots: Vec<WalkRoot>,
    /// The volumes that the plan of the whole machine skips, or `None` for a
    /// search of named paths.
    skipped_volumes: Option<Vec<SkippedMount>>,
}

/// Plans the search: the named paths, or the whole machine when there is none.
fn plan(cli: &Cli) -> anyhow::Result<Search> {
    if cli.paths.is_empty() {
        return plan_machine();
    }
    let mounts = if cli.one_file_system {
        mount_table()?
    } else {
        Vec::new()
    };
    let roots = cli
        .paths
        .iter()
        .map(|path| {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            named_root(path, &canonical, cli.one_file_system, &mounts)
        })
        .collect();
    Ok(Search {
        roots,
        skipped_volumes: None,
    })
}

#[cfg(target_os = "macos")]
fn plan_machine() -> anyhow::Result<Search> {
    let plan =
        ff::machine::plan_this_machine().context("cannot plan a search of the whole machine")?;
    Ok(Search {
        roots: plan.roots,
        skipped_volumes: Some(plan.skipped),
    })
}

#[cfg(not(target_os = "macos"))]
fn plan_machine() -> anyhow::Result<Search> {
    anyhow::bail!(
        "a search of the whole machine needs macOS. Name each directory to search after the pattern"
    )
}

#[cfg(target_os = "macos")]
fn mount_table() -> anyhow::Result<Vec<Mount>> {
    ff::machine::mount_table().context("cannot read the mount table")
}

/// Off macOS, `-x` stops at a mount point through the device check alone.
#[cfg(not(target_os = "macos"))]
fn mount_table() -> anyhow::Result<Vec<Mount>> {
    Ok(Vec::new())
}
