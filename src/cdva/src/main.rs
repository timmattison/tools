//! `cdva` — "count da various attributes".
//!
//! Counts the lines of code of a tree, as `cloc` does, and reports the test
//! code apart from the production code.
//!
//! This slice carries the command line, the walk, and the table. The numbers
//! it prints are both rules: a file whose path says it is test material is test
//! material from its first row to its last, and every other file of a language
//! with a tree rule is parsed so that the test nodes inside it are found.
//!
//! Five flags shape the table. `--by-file` prints one row for each file rather
//! than one for each language, `--sort` and `--top` order the rows and trim
//! them, and `--tests-only` and `--production-only` narrow every column to one
//! bucket. None of the five touches the total, which always covers every file
//! the walk counted.
//!
//! `--json` and `--csv` write the same report for a program to read rather
//! than a person. The five flags above still choose the rows, and none of them
//! narrows a machine format to fewer columns: a consumer that had to subtract
//! one bucket from the whole to reach the other is a consumer that will get it
//! wrong.
//!
//! A parse costs far more than a scan of the rows, so by default only a file
//! whose bytes hold a literal of its language ever reaches a parser.
//! `--no-tree` reads the path rule alone, which is the fast mode, and `--tree`
//! parses every file of a language that has a rule, which is the slow and
//! complete one. The two flags conflict, because asking for no parse and for
//! every parse at once is a mistake rather than a silent choice of one.
//!
//! # Two things this command states rather than assumes
//!
//! **A file the walk found twice is counted once.** Two roots that overlap —
//! `cdva . src` — hand the same file to the counter twice, and a total that
//! counted it twice reads exactly like a correct one. The key is the canonical
//! path, because the two roots reach the same file by two different names.
//!
//! **A file that cannot be read does not stop the run.** A tree of thousands
//! holds a file whose mode says no, and a counter that died on it would be a
//! counter nobody could use. The path and the reason go to standard error, the
//! file is left out of the table, and the run carries on.

use anyhow::Result;
use buildinfo::version_string;
use cdva::{
    render_csv, render_json, render_table, resolve_test_modules, walk, Bucket, Counter, FileCount,
    PathRules, ReportOptions, SortColumn, Summary, TreeMode, TreeRules, WalkOptions,
};
use clap::Parser;
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::PathBuf;

/// The path the tool counts when the command line names none.
const DEFAULT_PATH: &str = ".";

/// Count the lines of a tree, and report the test code apart from the
/// production code.
#[derive(Parser)]
#[command(
    name = "cdva",
    version = version_string!(),
    about = "Count da various attributes: count the lines of a tree, and report the test code apart from the production code"
)]
struct Cli {
    /// The paths to count.
    #[arg(value_name = "PATH", default_value = DEFAULT_PATH)]
    paths: Vec<PathBuf>,
    /// One row for each file, rather than one row for each language.
    #[arg(long)]
    by_file: bool,
    /// The column to order the rows by.
    #[arg(long, value_enum, default_value_t = SortColumn::Code)]
    sort: SortColumn,
    /// Keep only the first N rows. The total still covers every file.
    #[arg(long, value_name = "N")]
    top: Option<usize>,
    /// Report the test code alone.
    #[arg(long, conflicts_with = "production_only")]
    tests_only: bool,
    /// Report the production code alone.
    #[arg(long)]
    production_only: bool,
    /// Count a hidden file or directory.
    #[arg(long)]
    hidden: bool,
    /// Ignore every ignore file, including .gitignore.
    #[arg(long)]
    no_ignore: bool,
    /// Mark a path as test material. Repeat for more than one glob.
    #[arg(long, value_name = "GLOB")]
    test_glob: Vec<String>,
    /// Hold a path out of the test bucket. Repeat for more than one glob.
    #[arg(long, value_name = "GLOB")]
    production_glob: Vec<String>,
    /// Write the report as JSON.
    #[arg(long, conflicts_with = "csv")]
    json: bool,
    /// Write the report as CSV.
    #[arg(long)]
    csv: bool,
    /// Do not read any syntax tree. The path rule alone decides, which is fast.
    #[arg(long, conflicts_with = "tree")]
    no_tree: bool,
    /// Read the syntax tree of every file, skipping the fast literal
    /// pre-filter.
    #[arg(long)]
    tree: bool,
    /// Explain how one file was marked, span by span, rather than printing a
    /// table.
    ///
    /// The whole walk still runs, because one rule of the tool reads across
    /// files: the explanation of a file is the explanation of the number the
    /// table printed, and a file read on its own would answer a different
    /// question. Every flag that changes that answer therefore still applies.
    /// The flags that choose the rows of a table — --by-file, --sort, --top,
    /// --tests-only, and --production-only — do not, because this prints no
    /// table.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["json", "csv"])]
    explain: Option<PathBuf>,
}

impl Cli {
    /// How the report is shaped, as the five flags of the report say.
    const fn report_options(&self) -> ReportOptions {
        ReportOptions {
            by_file: self.by_file,
            bucket: self.bucket(),
            sort: self.sort,
            top: self.top,
        }
    }

    /// Which bucket the main columns report, as the two flags say.
    ///
    /// The flags conflict, so clap has already refused the pair by the time
    /// this reads them and no third answer is reachable here.
    const fn bucket(&self) -> Bucket {
        match (self.tests_only, self.production_only) {
            (true, _) => Bucket::TestsOnly,
            (_, true) => Bucket::ProductionOnly,
            _ => Bucket::Both,
        }
    }

    /// The report itself, as the two format flags choose it.
    ///
    /// A machine format carries every number the tool knows, whatever
    /// `--bucket` asked the table to print, and no footer or other prose: a
    /// stray line on standard output is a line every consumer has to strip.
    /// The row flags still choose which rows the report holds, because they
    /// choose rows and not columns.
    ///
    /// The two flags conflict, so clap has already refused the pair by the time
    /// this reads them and no third answer is reachable here.
    fn render(&self, summary: &Summary) -> String {
        let options = self.report_options();
        match (self.json, self.csv) {
            (true, _) => render_json(summary, options),
            (_, true) => render_csv(summary, options),
            _ => render_table(summary, options),
        }
    }

    /// When the tree rule runs, as the two flags say.
    ///
    /// The flags conflict, so clap has already refused the pair by the time
    /// this reads them and no third answer is reachable here.
    const fn tree_mode(&self) -> TreeMode {
        match (self.no_tree, self.tree) {
            (true, _) => TreeMode::Never,
            (_, true) => TreeMode::Always,
            _ => TreeMode::Auto,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let counter = Counter::new(PathRules::new(&cli.test_glob, &cli.production_glob)?)
        .with_tree_rules(TreeRules::new(), cli.tree_mode());
    let found = walk(
        &cli.paths,
        WalkOptions {
            hidden: cli.hidden,
            no_ignore: cli.no_ignore,
        },
    )?;

    let mut counted = count_all(&counter, &once_each(found));
    resolve_test_modules(&mut counted);
    print!("{}", cli.render(&Summary::new(counted)));

    Ok(())
}

/// Every file of the walk, with a file two roots both found kept once.
///
/// The canonical path is the key, because two overlapping roots reach one file
/// by two names and neither name tells the difference on its own. A path that
/// cannot be canonicalised — a broken symbolic link, a race with whoever is
/// writing the tree — answers for itself, so the file is still counted.
///
/// The first of the two entries is the one kept, so the order of the walk is the
/// order of the result and two runs over one tree read the same.
fn once_each(found: Vec<(PathBuf, PathBuf)>) -> Vec<(PathBuf, PathBuf)> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    found
        .into_iter()
        .filter(|(path, _)| {
            seen.insert(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        })
        .collect()
}

/// Counts every file, in parallel, and reports the ones that could not be read.
///
/// Reading a file and classifying it depend on nothing but that file, so the
/// files are independent and `rayon` splits them across the cores. The parallel
/// `filter_map` keeps the order of its input, so the table of a second run over
/// one tree is the table of the first.
fn count_all(counter: &Counter, files: &[(PathBuf, PathBuf)]) -> Vec<FileCount> {
    files
        .par_iter()
        .filter_map(
            |(path, relative)| match counter.count_path(path, relative) {
                Ok(counted) => counted,
                Err(error) => {
                    eprintln!("cdva: {error:#}. The file is not counted.");
                    None
                }
            },
        )
        .collect()
}
