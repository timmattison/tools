//! `tvfind` — find smart TVs on the local network and identify them.

mod cidr;
mod identify;
mod oui;
mod scan;
mod vendor;

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;

/// Find smart TVs on the local network
#[derive(Parser, Debug)]
#[clap(author, version = version_string!(), about)]
struct Args {
    /// Subnet to scan in CIDR form (default: the subnet of this machine)
    #[clap(long, value_name = "CIDR")]
    subnet: Option<String>,

    /// Only report TVs whose manufacturer contains this text, e.g. `tcl`
    #[clap(long, value_name = "NAME", default_value = "")]
    vendor: String,

    /// Skip the ARP cross-check that finds powered-off TVs
    #[clap(long)]
    no_arp: bool,
}

fn main() -> Result<()> {
    let _args = Args::parse();
    Ok(())
}
