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
use std::env;
use std::fs;
use std::net::Ipv4Addr;
use std::process::Command;

use anyhow::Result;
use buildinfo::version_string;
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use futures::stream::{self, StreamExt};
use reqwest::Client;

use identify::{Platform, Tv};
use scan::{probe, Probe, PROBE_PORTS};

/// Probes in flight at once. High enough to sweep a /23 in seconds, low enough
/// to stay well inside the open-file limit.
const MAX_CONCURRENT_PROBES: usize = 256;

/// Name `std::env::consts::OS` gives Linux.
const LINUX_OS: &str = "linux";

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
    eprintln!("Scanning {subnet} ({}) for TVs...", host_count(hosts.len()));

    let found = scan_hosts(&hosts, &args.vendor).await;
    report_tvs(&found.tvs);

    if !args.no_arp {
        report_powered_off(&found.answered, &args.vendor);
    }

    Ok(())
}

/// Describe a host count for the scan banner, e.g. `1 host` or `510 hosts`.
fn host_count(hosts: usize) -> String {
    let plural = if hosts == 1 { "" } else { "s" };
    format!("{hosts} host{plural}")
}

/// Heading for the powered-off report.
///
/// An OUI lookup identifies the company a hardware address block belongs to,
/// and nothing more, so the heading names that evidence rather than asserting
/// the device is a television.
fn powered_off_heading(vendor_filter: &str) -> String {
    let filter = vendor_filter.trim();
    let registrant = if filter.is_empty() {
        "a television maker".to_owned()
    } else {
        format!("a vendor that matches \"{filter}\"")
    };

    format!("\nRegistered to {registrant}, but answered no probe (powered off?):\n")
}

/// Hint printed when the `arp` command cannot be run.
///
/// `os` names the operating system, as `std::env::consts::OS` spells it. A
/// Linux distribution that ships iproute2 gets `arp` from the `net-tools`
/// package, and many such distributions do not install that package. Every
/// other system supplies `arp` itself, thus an absence there is a question of
/// the PATH.
fn missing_arp_hint(os: &str) -> &'static str {
    if os == LINUX_OS {
        "(install net-tools to get arp and also spot TVs that are powered off)"
    } else {
        "(put arp on the PATH to also spot TVs that are powered off)"
    }
}

/// What one sweep of the subnet found.
struct ScanResult {
    /// Televisions identified, after the vendor filter.
    tvs: Vec<Tv>,
    /// Every address that answered a probe, television or not.
    answered: HashSet<Ipv4Addr>,
}

impl ScanResult {
    /// Fold the probes of one sweep into televisions and answering addresses.
    ///
    /// The vendor filter narrows the televisions only. An address that answered
    /// stays in `answered` whatever the probe found there, because a host that
    /// completed a TCP handshake has power.
    fn from_probes(probes: Vec<Probe>, vendor_filter: &str) -> Self {
        let answered: HashSet<Ipv4Addr> = probes
            .iter()
            .filter(|found| found.answered)
            .map(|found| found.ip)
            .collect();

        let mut tvs: Vec<Tv> = probes
            .into_iter()
            .filter_map(|found| found.tv)
            .filter(|tv| vendor::matches(&tv.vendor, vendor_filter))
            .collect();
        tvs.sort_by_key(|tv| tv.ip);

        Self { tvs, answered }
    }
}

/// Probe every host on both TV ports and record what each probe found.
async fn scan_hosts(hosts: &[Ipv4Addr], vendor_filter: &str) -> ScanResult {
    let client = Client::new();

    let targets: Vec<(Ipv4Addr, u16, Platform)> = hosts
        .iter()
        .flat_map(|ip| {
            PROBE_PORTS
                .iter()
                .map(move |(port, platform)| (*ip, *port, *platform))
        })
        .collect();

    let probes: Vec<Probe> = stream::iter(targets)
        .map(|(ip, port, platform)| {
            let client = client.clone();
            async move { probe(&client, ip, port, platform).await }
        })
        .buffer_unordered(MAX_CONCURRENT_PROBES)
        .collect()
        .await;

    ScanResult::from_probes(probes, vendor_filter)
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
        table.add_row(tv.display_row());
    }

    println!("{table}");
}

/// Report neighbours whose MAC belongs to the wanted vendor but which answered
/// nothing — almost always a set that is powered off.
///
/// `answered` holds every address that answered a probe, and not only the
/// addresses that proved to be televisions. A host that answered has power, so
/// it is never powered off, whatever the probe found there.
///
/// Every step here is best-effort. A missing `arp` command and a missing OUI
/// database are not reasons to fail the scan. Each one prints one line that
/// names the tool, because a report that does not appear looks the same as a
/// network with no powered-off sets.
fn report_powered_off(answered: &HashSet<Ipv4Addr>, vendor_filter: &str) {
    let Some(arp_output) = arp_table() else {
        eprintln!("{}", missing_arp_hint(env::consts::OS));
        return;
    };
    let Some(db) = oui_database() else {
        eprintln!("(install nmap to also spot TVs that are powered off)");
        return;
    };

    let candidates = oui::unresponsive_candidates(&arp_output, &db, answered, vendor_filter);
    if candidates.is_empty() {
        return;
    }

    println!("{}", powered_off_heading(vendor_filter));
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

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::identify::{Platform, Tv};
    use super::scan::Probe;
    use super::{host_count, missing_arp_hint, powered_off_heading, ScanResult};

    /// A television as the Roku path reports one.
    fn a_tv(ip: Ipv4Addr, vendor: &str) -> Tv {
        Tv {
            ip,
            platform: Platform::RokuTv,
            vendor: vendor.to_owned(),
            model: "43S435".to_owned(),
            name: "Office".to_owned(),
            software: "15.0.4".to_owned(),
        }
    }

    #[test]
    fn counts_a_host_that_answered_without_proving_a_television_as_answered() {
        // A Roku streaming player answers the ECP port and is rejected as a
        // television. It has power, so it is not powered off.
        let player = Ipv4Addr::new(192, 168, 0, 77);
        let television = Ipv4Addr::new(192, 168, 0, 119);
        let silent = Ipv4Addr::new(192, 168, 0, 217);
        let probes = vec![
            Probe {
                ip: player,
                answered: true,
                tv: None,
            },
            Probe {
                ip: television,
                answered: true,
                tv: Some(a_tv(television, "TCL")),
            },
            Probe {
                ip: silent,
                answered: false,
                tv: None,
            },
        ];

        let found = ScanResult::from_probes(probes, "");

        assert!(
            found.answered.contains(&player),
            "a Roku streaming player has power"
        );
        assert!(found.answered.contains(&television));
        assert!(
            !found.answered.contains(&silent),
            "a host that refused the handshake answered nothing"
        );
    }

    #[test]
    fn keeps_a_television_the_vendor_filter_removed_among_the_addresses_that_answered() {
        let sony = Ipv4Addr::new(192, 168, 0, 33);
        let probes = vec![Probe {
            ip: sony,
            answered: true,
            tv: Some(a_tv(sony, "Sony")),
        }];

        let found = ScanResult::from_probes(probes, "tcl");

        assert!(found.tvs.is_empty(), "the filter keeps a Sony set out");
        assert!(found.answered.contains(&sony), "the set still has power");
    }

    #[test]
    fn orders_the_televisions_by_address() {
        let high = Ipv4Addr::new(192, 168, 0, 248);
        let low = Ipv4Addr::new(192, 168, 0, 119);
        let probes = vec![
            Probe {
                ip: high,
                answered: true,
                tv: Some(a_tv(high, "TCL")),
            },
            Probe {
                ip: low,
                answered: true,
                tv: Some(a_tv(low, "TCL")),
            },
        ];

        let found = ScanResult::from_probes(probes, "");

        assert_eq!(
            found.tvs.iter().map(|tv| tv.ip).collect::<Vec<_>>(),
            vec![low, high]
        );
    }

    #[test]
    fn says_what_an_unfiltered_candidate_actually_has_in_common_with_a_tv() {
        // The evidence is the address block, not the device. Saying more than
        // that is what put a router and a speaker under a "probably a TV" list.
        let heading = powered_off_heading("");

        assert!(
            heading.contains("television maker"),
            "the heading must name the evidence, got {heading:?}"
        );
        assert!(!heading.contains("Probably a TV"));
    }

    #[test]
    fn names_the_filter_the_user_gave_in_the_heading() {
        let heading = powered_off_heading("tcl");

        assert!(heading.contains("tcl"), "got {heading:?}");
    }

    #[test]
    fn names_the_package_that_supplies_arp_on_linux() {
        // A distribution that ships iproute2 carries no arp command until
        // net-tools is installed, and the powered-off report needs arp.
        let hint = missing_arp_hint("linux");

        assert!(hint.contains("net-tools"), "got {hint:?}");
        assert!(hint.contains("arp"), "got {hint:?}");
        assert!(
            hint.contains("powered off"),
            "the hint must name the report that was lost, got {hint:?}"
        );
    }

    #[test]
    fn points_at_the_path_on_a_system_that_supplies_arp_itself() {
        // macOS installs arp with the system, so there is no package to name.
        let hint = missing_arp_hint("macos");

        assert!(hint.contains("arp"), "got {hint:?}");
        assert!(hint.contains("PATH"), "got {hint:?}");
        assert!(
            !hint.contains("net-tools"),
            "macOS has no net-tools package, got {hint:?}"
        );
    }

    #[test]
    fn describes_a_single_host_in_the_singular() {
        assert_eq!(host_count(1), "1 host");
    }

    #[test]
    fn describes_several_hosts_in_the_plural() {
        assert_eq!(host_count(510), "510 hosts");
    }

    #[test]
    fn describes_an_empty_range_in_the_plural() {
        assert_eq!(host_count(0), "0 hosts");
    }
}
