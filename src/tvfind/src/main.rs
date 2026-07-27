//! `tvfind` — find smart TVs on the local network and identify them.
//!
//! Televisions are addressed directly on the two ports their firmware answers
//! on rather than discovered over SSDP or mDNS. Access points routinely filter
//! multicast between radios, which hides sets that reply perfectly well when
//! asked directly.

mod cidr;
mod identify;
mod oui;
mod scan;
mod vendor;

use std::collections::HashSet;
use std::fs;
use std::net::Ipv4Addr;
use std::process::Command;

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use futures::stream::{self, StreamExt};
use reqwest::Client;

use identify::Tv;
use scan::{fetch_google_tv, fetch_roku, is_port_open, PROBE_PORTS, ROKU_ECP_PORT};

/// Probes in flight at once. High enough to sweep a /23 in seconds, low enough
/// to stay well inside the open-file limit.
const MAX_CONCURRENT_PROBES: usize = 256;

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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let subnet = match args.subnet {
        Some(ref explicit) => explicit.clone(),
        None => cidr::local_subnet()?,
    };
    let hosts = cidr::hosts_in(&subnet)?;
    eprintln!("Scanning {subnet} ({} hosts) for TVs...", hosts.len());

    let tvs = find_tvs(&hosts, &args.vendor).await;
    report_tvs(&tvs);

    if !args.no_arp {
        report_powered_off(&tvs, &args.vendor);
    }

    Ok(())
}

/// Probe every host on both TV ports and identify whatever answers.
async fn find_tvs(hosts: &[Ipv4Addr], vendor_filter: &str) -> Vec<Tv> {
    let client = Client::new();

    let targets: Vec<(Ipv4Addr, u16)> = hosts
        .iter()
        .flat_map(|ip| PROBE_PORTS.iter().map(move |port| (*ip, *port)))
        .collect();

    let mut tvs: Vec<Tv> = stream::iter(targets)
        .map(|(ip, port)| {
            let client = client.clone();
            async move {
                if !is_port_open(ip, port).await {
                    return None;
                }
                let base_url = format!("http://{ip}:{port}");
                if port == ROKU_ECP_PORT {
                    fetch_roku(&client, &base_url, ip).await
                } else {
                    fetch_google_tv(&client, &base_url, ip).await
                }
            }
        })
        .buffer_unordered(MAX_CONCURRENT_PROBES)
        .filter_map(|found| async move { found })
        .collect()
        .await;

    tvs.retain(|tv| vendor::matches(&tv.vendor, vendor_filter));
    tvs.sort_by_key(|tv| tv.ip);
    tvs
}

/// Print the identified televisions as a table.
fn report_tvs(tvs: &[Tv]) {
    if tvs.is_empty() {
        println!("No TVs answered.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "IP", "Name", "Vendor", "Model", "Platform", "Software",
        ]);

    for tv in tvs {
        table.add_row(vec![
            tv.ip.to_string(),
            tv.name.clone(),
            tv.vendor.clone(),
            tv.model.clone(),
            tv.platform.label().to_owned(),
            tv.software.clone(),
        ]);
    }

    println!("{table}");
}

/// Report neighbours whose MAC belongs to the wanted vendor but which answered
/// nothing — almost always a set that is powered off.
///
/// Every step here is best-effort: without `arp` or nmap's OUI database there
/// is simply nothing extra to say, which is not a reason to fail the scan.
fn report_powered_off(tvs: &[Tv], vendor_filter: &str) {
    let Some(arp_output) = arp_table() else {
        return;
    };
    let Some(db) = oui_database() else {
        eprintln!("(install nmap to also spot TVs that are powered off)");
        return;
    };

    let identified: HashSet<Ipv4Addr> = tvs.iter().map(|tv| tv.ip).collect();
    let candidates = oui::unresponsive_candidates(&arp_output, &db, &identified, vendor_filter);
    if candidates.is_empty() {
        return;
    }

    println!("\nProbably a TV but answering nothing (powered off?):\n");
    for candidate in candidates {
        println!(
            "  {:<16} {:<19} {}",
            candidate.ip, candidate.mac, candidate.vendor
        );
    }
}

/// The system ARP table, or `None` if `arp` is unavailable.
fn arp_table() -> Option<String> {
    let output = Command::new("arp").args(["-a", "-n"]).output().ok()?;
    String::from_utf8(output.stdout).ok()
}

/// nmap's OUI database from the first path that exists.
fn oui_database() -> Option<std::collections::HashMap<String, String>> {
    oui::OUI_DB_PATHS
        .iter()
        .find_map(|path| fs::read_to_string(path).ok())
        .map(|text| oui::parse_db(&text))
}
