//! `grist` - work out which squash-merge order costs the least to resolve.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, ContentArrangement, Table};
use grist::{orderings_to_simulate, BranchName, OrderingScore, Simulator};

// No `about` in the attribute below, and the absence is the point. clap's
// derive takes the doc comment as the help text. A bare `about` takes
// `CARGO_PKG_DESCRIPTION` instead, and the doc comment then reaches nobody.
// Two sentences describe the tool, one of them is dead, and the dead one sits
// where a developer edits the help. The manifest keeps its own sentence, which
// crates.io and `cargo search` show to a reader who has run nothing.
//
// One line only. A second paragraph here becomes clap's `long_about`, which
// `--help` prints and `-h` does not, and the two spellings of one switch then
// answer differently.
/// Rank the orders you could squash-merge branches in, cheapest conflicts first
#[derive(Parser, Debug)]
#[clap(author, version = version_string!())]
struct Args {
    /// Branches to land, in any order
    #[clap(required = true, value_name = "BRANCH")]
    branches: Vec<String>,

    /// Ref the branches are landing on top of
    #[clap(long, default_value = "HEAD", value_name = "REF")]
    onto: String,

    /// Print only the winning order, space separated, for piping
    #[clap(short, long)]
    quiet: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let repo = std::env::current_dir().context("could not determine the current directory")?;
    let branches: Vec<BranchName> = args.branches.iter().map(BranchName::new).collect();

    // Before the announcement below, because a run that cannot start must not be
    // advertised. Building the simulator runs `gitscratch`'s pre-flight, so
    // somewhere outside every repository is refused here, by name, rather than
    // arriving later as git's complaint from inside `worktree add`.
    let mut simulator = Simulator::new(&repo, &args.onto)?;
    if !args.quiet {
        // Ask the library what the run costs before announcing one: it is the
        // same check `evaluate` makes, so a list grist will not simulate is
        // turned away here rather than advertised and then refused.
        let orderings = orderings_to_simulate(&branches)?;
        eprintln!(
            "Simulating {orderings} ordering{} of {} branches onto {}...",
            if orderings == 1 { "" } else { "s" },
            branches.len(),
            args.onto
        );
        simulator = simulator.with_progress(|message| eprintln!("  {message}"));
    }

    let ranked = simulator.evaluate(&branches)?;

    let winner = ranked
        .first()
        .context("no orderings were evaluated")?
        .order()
        .iter()
        .map(BranchName::to_string)
        .collect::<Vec<_>>()
        .join(" ");

    if args.quiet {
        println!("{winner}");
        return Ok(());
    }

    eprintln!();
    println!("{}", render(&ranked));
    println!();
    println!("Land them in this order: {winner}");

    // Only say the order does not matter when it genuinely does not. The whole
    // ranking key has to match: orderings can share a hunk count and still be
    // ranked apart on stops or files, and shrugging at a difference grist just
    // printed a winner for contradicts the line above.
    let cheapest = ranked[0].cost_key();
    if ranked.iter().all(|score| score.cost_key() == cheapest) {
        println!("Every order costs the same, so pick whichever you prefer.");
    }

    Ok(())
}

/// Render the ranked orderings as a table.
fn render(ranked: &[OrderingScore]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["", "Order", "Hunks", "Stops", "Files"]);

    let highlight = std::io::stdout().is_terminal();

    for (index, score) in ranked.iter().enumerate() {
        let order = score
            .order()
            .iter()
            .map(BranchName::to_string)
            .collect::<Vec<_>>()
            .join(" \u{2192} ");

        let marker = if index == 0 { "\u{2713}" } else { "" };
        let cells = [
            marker.to_string(),
            order,
            score.hunks().digits(),
            score.stops().digits(),
            score.files().digits(),
        ];

        let row: Vec<Cell> = cells
            .into_iter()
            .map(|text| {
                let cell = Cell::new(text);
                if index == 0 && highlight {
                    cell.fg(comfy_table::Color::Green)
                } else {
                    cell
                }
            })
            .collect();

        table.add_row(row);
    }

    table
}
