use buildinfo::version_string;
use clap::Parser;

mod age;
mod bar;
mod git;
mod render;

#[derive(Parser)]
#[command(name = "gsw")]
#[command(version = version_string!())]
#[command(
    about = "Compact git status watch — one-shot pretty output for use with viddy",
    long_about = "Prints a compact, color-coded view of the current branch's state: \
                  commits ahead of the base branch, last-commit age, and a per-file \
                  list showing a magnitude bar, +/- counts, and recency. Designed to \
                  be run repeatedly under `viddy`."
)]
struct Cli {
    /// Strip ANSI color codes from output.
    #[arg(long)]
    no_color: bool,

    /// Base ref to compare against (default: main, then master, then origin/HEAD).
    #[arg(long)]
    base: Option<String>,

    /// Maximum number of file rows to show (default: fit terminal).
    #[arg(long)]
    max_files: Option<usize>,

    /// Width of the magnitude bar in cells.
    #[arg(long, default_value_t = 6)]
    bar_width: usize,
}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
