//! Command line entrance for `thermal-watch`.

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;

/// Show whether this Mac decreases its clock under sustained load.
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// Make a full P-core load instead of watching a load you started
    #[clap(long)]
    load: bool,

    /// How long to watch, in seconds
    #[clap(long, default_value_t = 300)]
    duration: u64,

    /// Print one JSON object for each sample instead of a live display
    #[clap(long)]
    json: bool,
}

fn main() -> Result<()> {
    let _args = Args::parse();
    anyhow::bail!("not implemented")
}
