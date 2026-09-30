use anyhow::Result;
use clap::Subcommand;
use futures::stream::StreamExt;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    device_helper::get_device_id_or_prompt,
    models::{Device, DeviceAction, DeviceDetails, DeviceStatistics, Page, PortAction},
    output::{
        print_output, print_vec_table, render_collection, render_output, render_page_listing,
        OutputFormat, Report,
    },
    pagination::fetch_all,
    site_helper::get_site_id_or_prompt,
};

/// Shown in place of a figure the controller did not report.
const NO_DATA: &str = "N/A";

/// Shown in the table in place of every figure of a device whose statistics
/// request failed, so an unreachable device or a permissions problem cannot be
/// mistaken for a device that simply had nothing to report. The JSON document
/// gives the reason for the failure instead.
const FETCH_FAILED: &str = "ERROR";

#[derive(Subcommand, Debug)]
pub enum DevicesCommand {
    /// List devices on a site
    List {
        /// Maximum number of devices to return
        #[clap(long, default_value = "25")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,
    },

    /// Get device details
    Get {
        /// Device ID
        device_id: Uuid,
    },

    /// Get device statistics
    Stats {
        /// Device ID (if not provided, will show device list)
        #[clap(conflicts_with = "all")]
        device_id: Option<Uuid>,

        /// Show statistics for all devices
        #[clap(long)]
        all: bool,
    },

    /// Restart a device
    Restart {
        /// Device ID
        device_id: Uuid,
    },

    /// Power cycle a port
    PowerCyclePort {
        /// Device ID
        device_id: Uuid,

        /// Port index
        port_idx: u32,
    },
}

#[derive(Tabled, serde::Serialize)]
pub struct DeviceRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Model")]
    model: String,
    #[tabled(rename = "MAC Address")]
    mac_address: String,
    #[tabled(rename = "IP Address")]
    ip_address: String,
    #[tabled(rename = "State")]
    state: String,
}

impl From<&Device> for DeviceRow {
    fn from(device: &Device) -> Self {
        Self {
            id: device.id.to_string(),
            name: device.name.clone(),
            model: device.model.clone(),
            mac_address: device.mac_address.to_string(),
            ip_address: device.ip_address.to_string(),
            // The controller's own spelling, which is the only rendering that
            // still says something when the state is one this build has never
            // seen.
            state: device.state.to_string(),
        }
    }
}

#[derive(Tabled, serde::Serialize)]
pub struct DeviceStatsRow {
    #[tabled(rename = "Uptime")]
    uptime: String,
    #[tabled(rename = "CPU %")]
    cpu_pct: String,
    #[tabled(rename = "Memory %")]
    memory_pct: String,
    #[tabled(rename = "Load Avg (1m)")]
    load_avg_1m: String,
    #[tabled(rename = "Load Avg (5m)")]
    load_avg_5m: String,
    #[tabled(rename = "Load Avg (15m)")]
    load_avg_15m: String,
    #[tabled(rename = "TX Rate")]
    tx_rate: String,
    #[tabled(rename = "RX Rate")]
    rx_rate: String,
}

fn format_uptime(seconds: Option<u64>) -> String {
    match seconds {
        Some(secs) => {
            let days = secs / 86400;
            let hours = (secs % 86400) / 3600;
            let minutes = (secs % 3600) / 60;

            if days > 0 {
                format!("{}d {}h {}m", days, hours, minutes)
            } else if hours > 0 {
                format!("{}h {}m", hours, minutes)
            } else {
                format!("{}m", minutes)
            }
        }
        None => NO_DATA.to_string(),
    }
}

fn format_rate(bps: Option<u64>) -> String {
    match bps {
        Some(rate) => {
            if rate >= 1_000_000_000 {
                format!("{:.1} Gbps", rate as f64 / 1_000_000_000.0)
            } else if rate >= 1_000_000 {
                format!("{:.1} Mbps", rate as f64 / 1_000_000.0)
            } else if rate >= 1_000 {
                format!("{:.1} Kbps", rate as f64 / 1_000.0)
            } else {
                format!("{} bps", rate)
            }
        }
        None => NO_DATA.to_string(),
    }
}

fn format_percentage(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{:.1}%", p),
        None => NO_DATA.to_string(),
    }
}

fn format_load_avg(load: Option<f64>) -> String {
    match load {
        Some(l) => format!("{:.2}", l),
        None => NO_DATA.to_string(),
    }
}

impl From<&crate::models::DeviceStatistics> for DeviceStatsRow {
    fn from(stats: &crate::models::DeviceStatistics) -> Self {
        Self {
            uptime: format_uptime(stats.uptime_sec),
            cpu_pct: format_percentage(stats.cpu_utilization_pct),
            memory_pct: format_percentage(stats.memory_utilization_pct),
            load_avg_1m: format_load_avg(stats.load_average_1min),
            load_avg_5m: format_load_avg(stats.load_average_5min),
            load_avg_15m: format_load_avg(stats.load_average_15min),
            tx_rate: format_rate(stats.uplink.as_ref().and_then(|u| u.tx_rate_bps)),
            rx_rate: format_rate(stats.uplink.as_ref().and_then(|u| u.rx_rate_bps)),
        }
    }
}

#[derive(Tabled, serde::Serialize)]
pub struct DeviceStatsRowWithName {
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Model")]
    model: String,
    #[tabled(rename = "Uptime")]
    uptime: String,
    #[tabled(rename = "CPU %")]
    cpu_pct: String,
    #[tabled(rename = "Memory %")]
    memory_pct: String,
    #[tabled(rename = "Load Avg (1m)")]
    load_avg_1m: String,
    #[tabled(rename = "TX Rate")]
    tx_rate: String,
    #[tabled(rename = "RX Rate")]
    rx_rate: String,
}

impl DeviceStatsRowWithName {
    /// Build the row for one device.
    ///
    /// `stats` is the outcome of that device's statistics request: `Err` means
    /// the request itself failed, which is a different thing from a device
    /// that answered without any figures to report.
    fn from_device_and_stats(
        device: &Device,
        stats: Result<&crate::models::DeviceStatistics, &anyhow::Error>,
    ) -> Self {
        match stats {
            Ok(s) => Self {
                name: device.name.clone(),
                model: device.model.clone(),
                uptime: format_uptime(s.uptime_sec),
                cpu_pct: format_percentage(s.cpu_utilization_pct),
                memory_pct: format_percentage(s.memory_utilization_pct),
                load_avg_1m: format_load_avg(s.load_average_1min),
                tx_rate: format_rate(s.uplink.as_ref().and_then(|u| u.tx_rate_bps)),
                rx_rate: format_rate(s.uplink.as_ref().and_then(|u| u.rx_rate_bps)),
            },
            Err(_) => Self {
                name: device.name.clone(),
                model: device.model.clone(),
                uptime: FETCH_FAILED.to_string(),
                cpu_pct: FETCH_FAILED.to_string(),
                memory_pct: FETCH_FAILED.to_string(),
                load_avg_1m: FETCH_FAILED.to_string(),
                tx_rate: FETCH_FAILED.to_string(),
                rx_rate: FETCH_FAILED.to_string(),
            },
        }
    }
}

/// One device of a site, together with the outcome of its statistics request.
struct DeviceStatsOutcome {
    /// The device, as the device list gave it.
    device: Device,
    /// `Err` means the request itself failed. That is a different thing from
    /// a device that answered with no figures to report.
    statistics: Result<DeviceStatistics>,
}

/// One device in the JSON document of `devices stats --all`.
///
/// The statistics keep the field names and the units of [`DeviceStatistics`],
/// so a program that reads `devices stats <id> --output json` reads each entry
/// too.
#[derive(serde::Serialize)]
struct DeviceStatsEntry<'a> {
    id: Uuid,
    name: &'a str,
    model: &'a str,
    #[serde(flatten)]
    outcome: DeviceStatsEntryOutcome<'a>,
}

/// The statistics of a [`DeviceStatsEntry`], or the reason it has none.
///
/// Each variant becomes one key of the entry, `statistics` or `error`. A
/// program tells a failed device from a healthy one by the key it finds.
#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum DeviceStatsEntryOutcome<'a> {
    /// The statistics, as the controller gave them.
    Statistics(&'a DeviceStatistics),
    /// The reason the statistics request failed.
    Error(String),
}

impl<'a> From<&'a DeviceStatsOutcome> for DeviceStatsEntry<'a> {
    fn from(fetched: &'a DeviceStatsOutcome) -> Self {
        Self {
            id: fetched.device.id,
            name: &fetched.device.name,
            model: &fetched.device.model,
            outcome: match &fetched.statistics {
                Ok(statistics) => DeviceStatsEntryOutcome::Statistics(statistics),
                Err(error) => DeviceStatsEntryOutcome::Error(failure_reason(error)),
            },
        }
    }
}

/// The reason a statistics request failed, with the chain of causes.
///
/// The note on standard error and the JSON entry give the same text.
///
/// # Arguments
///
/// * `error` - The error of the failed request.
///
/// # Returns
///
/// The reason, with each cause after the error it explains.
fn failure_reason(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

pub async fn handle_devices_command(
    command: DevicesCommand,
    site_id: Option<Uuid>,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        DevicesCommand::List { limit, offset } => {
            list_devices(client, site_id, limit, offset, output_format)
                .await
                .map(Report::print)
        }
        DevicesCommand::Get { device_id } => {
            get_device(client, site_id, device_id, output_format).await
        }
        DevicesCommand::Stats { device_id, all } => {
            get_device_stats(client, site_id, device_id, all, output_format).await
        }
        DevicesCommand::Restart { device_id } => restart_device(client, site_id, device_id)
            .await
            .map(Report::print),
        DevicesCommand::PowerCyclePort {
            device_id,
            port_idx,
        } => power_cycle_port(client, site_id, device_id, port_idx)
            .await
            .map(Report::print),
    }
}

/// List one page of the devices on a site.
///
/// # Arguments
///
/// * `client` - The controller client to list the devices with.
/// * `site_id` - The site the user named, if any.
/// * `limit` - How many devices to ask for.
/// * `offset` - Where in the collection the page starts.
/// * `output_format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered page, ready to print.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, if the request fails, or
/// if the answer cannot be rendered.
async fn list_devices(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    limit: u32,
    offset: u64,
    output_format: OutputFormat,
) -> Result<Report> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let params: Vec<(&str, &dyn std::fmt::Display)> =
        vec![("limit", &limit_str), ("offset", &offset_str)];

    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/devices", site_id);
    let page: Page<Device> = client.get_with_params(&path, &params).await?;

    render_page_listing::<Device, DeviceRow>(&page, output_format)
}

async fn get_device(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/devices/{}", site_id, device_id);
    let device: DeviceDetails = client.get(&path).await?;

    print_output(&device, output_format)?;
    Ok(())
}

async fn get_device_stats(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Option<Uuid>,
    all: bool,
    output_format: OutputFormat,
) -> Result<()> {
    // `--all` and a device id are declared as conflicting arguments, so an
    // invocation carrying both never reaches this point and no request is
    // issued to reject it.
    let site_id = get_site_id_or_prompt(client, site_id).await?;

    if all {
        // Get stats for all devices
        get_all_device_stats(client, site_id, output_format)
            .await
            .map(Report::print)
    } else if let Some(device_id) = device_id {
        // Get stats for specific device
        get_single_device_stats(client, site_id, device_id, output_format).await
    } else {
        // No device named: let the user pick one
        let device_id = get_device_id_or_prompt(client, site_id, None).await?;
        get_single_device_stats(client, site_id, device_id, output_format).await
    }
}

async fn get_single_device_stats(
    client: &UnifiClient,
    site_id: Uuid,
    device_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let path = format!("sites/{}/devices/{}/statistics/latest", site_id, device_id);
    let stats: DeviceStatistics = client.get(&path).await?;

    match output_format {
        OutputFormat::Json => {
            print_output(&stats, output_format)?;
        }
        OutputFormat::Table => {
            let stats_row = DeviceStatsRow::from(&stats);
            print_vec_table(&[stats_row], output_format)?;
        }
    }

    Ok(())
}

/// Fetch and render the statistics of every device on a site.
///
/// # Arguments
///
/// * `client` - The controller client to fetch the statistics with.
/// * `site_id` - The site whose devices to report on.
/// * `output_format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered statistics, ready to print.
///
/// # Errors
///
/// Returns an error if the device list cannot be fetched, or if the answer
/// cannot be rendered. A failed statistics request for one device is not an
/// error: its row or its JSON entry says so instead.
async fn get_all_device_stats(
    client: &UnifiClient,
    site_id: Uuid,
    output_format: OutputFormat,
) -> Result<Report> {
    let devices_path = format!("sites/{}/devices", site_id);
    let devices: Vec<Device> = fetch_all(client, &devices_path).await?;

    let statistics = fetch_device_statistics(&devices, |device| {
        let stats_path = format!("sites/{}/devices/{}/statistics/latest", site_id, device.id);
        async move { client.get::<DeviceStatistics>(&stats_path).await }
    })
    .await;

    let fetched: Vec<DeviceStatsOutcome> = devices
        .into_iter()
        .zip(statistics)
        .map(|(device, statistics)| {
            if let Err(error) = &statistics {
                eprintln!(
                    "Failed to fetch statistics for {} ({}): {}",
                    device.name,
                    device.id,
                    failure_reason(error)
                );
            }
            DeviceStatsOutcome { device, statistics }
        })
        .collect();

    all_device_stats_report(&fetched, output_format)
}

/// Upper bound on statistics requests in flight at once.
///
/// A site can hold hundreds of devices and the controller answering them is
/// often a home router, so the fetches are throttled rather than all launched
/// at once.
const MAX_CONCURRENT_STAT_REQUESTS: usize = 8;

/// Fetch the statistics of every device, one result per device in device
/// order.
///
/// A failed fetch is reported as an `Err` in the corresponding slot rather
/// than aborting the whole listing: one unreachable device should not hide the
/// rest of the site.
///
/// # Arguments
///
/// * `devices` - The devices to fetch statistics for.
/// * `fetch` - Issues the statistics request for one device.
///
/// # Returns
///
/// One result per device, in the same order as `devices`, however the
/// individual requests happened to finish.
async fn fetch_device_statistics<F, Fut>(
    devices: &[Device],
    fetch: F,
) -> Vec<Result<DeviceStatistics>>
where
    F: Fn(&Device) -> Fut,
    Fut: std::future::Future<Output = Result<DeviceStatistics>>,
{
    futures::stream::iter(devices.iter().map(&fetch))
        .buffered(MAX_CONCURRENT_STAT_REQUESTS)
        .collect()
        .await
}

/// Render the per-device statistics of `devices stats --all`.
///
/// A table gives each figure as text for a person. JSON gives one
/// [`DeviceStatsEntry`] per device: the statistics as the controller gave
/// them, or the reason the request failed.
///
/// # Arguments
///
/// * `fetched` - One outcome per device, in device order.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered statistics, together with the note for a site that holds no
/// devices.
///
/// # Errors
///
/// Returns an error if the statistics cannot be serialized.
fn all_device_stats_report(fetched: &[DeviceStatsOutcome], format: OutputFormat) -> Result<Report> {
    let report = match format {
        OutputFormat::Table => {
            let rows: Vec<DeviceStatsRowWithName> = fetched
                .iter()
                .map(|outcome| {
                    DeviceStatsRowWithName::from_device_and_stats(
                        &outcome.device,
                        outcome.statistics.as_ref(),
                    )
                })
                .collect();
            render_collection(&rows, format)?
        }
        // A site with no devices gives `[]`, the same document as every other
        // collection that holds nothing.
        OutputFormat::Json => {
            let entries: Vec<DeviceStatsEntry<'_>> =
                fetched.iter().map(DeviceStatsEntry::from).collect();
            Report::of_document(render_output(&entries, format)?)
        }
    };

    if fetched.is_empty() {
        return Ok(report.with_note("No devices found on this site."));
    }

    Ok(report)
}

/// Ask the controller to restart a device.
///
/// # Arguments
///
/// * `client` - The controller client to send the request with.
/// * `site_id` - The site the user named, if any.
/// * `device_id` - The device to restart.
///
/// # Returns
///
/// The report that says the controller accepted the request.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, or if the request fails.
async fn restart_device(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Uuid,
) -> Result<Report> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/devices/{}/actions", site_id, device_id);
    let action = DeviceAction::Restart;

    let _: serde_json::Value = client.post(&path, &action).await?;
    Ok(Report::of_note("Device restart initiated successfully"))
}

/// Ask the controller to power cycle one port of a device.
///
/// # Arguments
///
/// * `client` - The controller client to send the request with.
/// * `site_id` - The site the user named, if any.
/// * `device_id` - The device that holds the port.
/// * `port_idx` - The index of the port to power cycle.
///
/// # Returns
///
/// The report that says the controller accepted the request.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, or if the request fails.
async fn power_cycle_port(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Uuid,
    port_idx: u32,
) -> Result<Report> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!(
        "sites/{}/devices/{}/interfaces/ports/{}/actions",
        site_id, device_id, port_idx
    );
    let action = PortAction::PowerCycle;

    let _: serde_json::Value = client.post(&path, &action).await?;
    Ok(Report::of_note("Port power cycle initiated successfully"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DeviceFeature, DeviceInterface, DeviceInterfaceStatistics, DeviceState, IpAddress,
        MacAddress, UplinkStatistics,
    };
    use crate::test_support::parse_args_for_test;
    use std::cell::Cell;

    /// Box-drawing corner produced by the table renderer.
    const TABLE_CORNER: char = '┌';

    /// A device with just enough shape to build a statistics row from.
    fn test_device(name: &str) -> Device {
        Device {
            id: Uuid::new_v4(),
            name: name.to_string(),
            model: "U6-LR".to_string(),
            mac_address: MacAddress::from("00:11:22:33:44:55"),
            ip_address: IpAddress::from("192.168.1.2"),
            state: DeviceState::Online,
            features: vec![DeviceFeature::AccessPoint],
            interfaces: vec![DeviceInterface::Radios],
        }
    }

    /// One device per name, each with statistics that its request fetched.
    fn outcomes_for(names: &[&str]) -> Vec<DeviceStatsOutcome> {
        names
            .iter()
            .map(|name| DeviceStatsOutcome {
                device: test_device(name),
                statistics: Ok(stats_with_uptime(0)),
            })
            .collect()
    }

    /// `--output json` is an explicit request from the user, usually because
    /// the output is being piped into something. Answering it with a table
    /// silently breaks that pipeline.
    #[test]
    fn all_device_stats_honour_the_json_output_format() {
        let fetched = outcomes_for(&["ap-lr", "switch-8"]);

        let report = all_device_stats_report(&fetched, OutputFormat::Json)
            .expect("rendering the all-devices stats must succeed");
        let rendered = document_of(&report);
        let parsed: serde_json::Value = serde_json::from_str(rendered).unwrap_or_else(|error| {
            panic!("--output json must produce JSON ({error}), got:\n{rendered}")
        });

        let entries = parsed
            .as_array()
            .unwrap_or_else(|| panic!("--output json must produce an array, got:\n{rendered}"));
        assert_eq!(entries.len(), 2, "every device must appear:\n{rendered}");
        assert_eq!(
            entries[0]["name"], "ap-lr",
            "wrong first device:\n{rendered}"
        );
        assert_eq!(
            entries[1]["name"], "switch-8",
            "wrong second device:\n{rendered}"
        );
    }

    /// A device that answered with nothing to report and a device whose
    /// statistics request failed outright are different situations -- one is
    /// idle or offline, the other may be a permissions problem -- and a row
    /// that reads "N/A" either way tells the user nothing about which.
    #[test]
    fn a_failed_fetch_reads_differently_from_a_device_with_no_figures() {
        let device = test_device("ap-lr");
        let nothing_to_report = stats_with_no_figures();
        let failure = anyhow::anyhow!("HTTP 403: insufficient permissions");

        let no_figures =
            DeviceStatsRowWithName::from_device_and_stats(&device, Ok(&nothing_to_report));
        let failed = DeviceStatsRowWithName::from_device_and_stats(&device, Err(&failure));

        assert_eq!(
            no_figures.uptime, NO_DATA,
            "a device with nothing to report keeps reading N/A"
        );
        assert_ne!(
            failed.uptime, no_figures.uptime,
            "a failed statistics request must not be rendered as missing data"
        );
        assert_ne!(
            failed.cpu_pct, no_figures.cpu_pct,
            "a failed statistics request must not be rendered as missing data"
        );
        assert_eq!(
            failed.name, device.name,
            "a failed device must still be listed by name"
        );
    }

    /// Statistics from a device that answered without any figures.
    fn stats_with_no_figures() -> DeviceStatistics {
        DeviceStatistics {
            uptime_sec: None,
            ..stats_with_uptime(0)
        }
    }

    /// A state this build has never heard of still means something to the
    /// user -- it is whatever the controller called it -- so the listing must
    /// show that word rather than a placeholder or a debug rendering of the
    /// wrapper it landed in.
    #[test]
    fn an_unknown_device_state_is_listed_as_the_controller_spelled_it() {
        const FUTURE_STATE: &str = "FUTURE_FIRMWARE_VALUE";
        let json = format!(
            r#"{{
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "ap-lr", "model": "U6-LR",
                "macAddress": "00:11:22:33:44:55", "ipAddress": "192.168.1.2",
                "state": "{FUTURE_STATE}",
                "features": [], "interfaces": []
            }}"#
        );
        let device: Device = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("an unfamiliar state must still parse: {error}"));

        let row = DeviceRow::from(&device);

        assert_eq!(
            row.state, FUTURE_STATE,
            "the listing must show the state the controller reported"
        );
    }

    /// A state this build does know keeps the controller's own spelling too,
    /// so the column reads consistently whichever kind of value it holds.
    #[test]
    fn a_known_device_state_is_listed_as_the_controller_spells_it() {
        let row = DeviceRow::from(&test_device("ap-lr"));

        assert_eq!(
            row.state, "ONLINE",
            "the listing must show the state the controller reported"
        );
    }

    /// A device id together with `--all` is a contradiction, and it is one
    /// clap can see before anything talks to the controller. Catching it at
    /// runtime instead means resolving the site first -- a round trip that can
    /// fail on its own and report the wrong problem entirely.
    #[test]
    fn stats_rejects_a_device_id_together_with_all_at_parse_time() {
        let device_id = Uuid::new_v4().to_string();

        let error = parse_args_for_test(["ufa", "devices", "stats", "--all", &device_id])
            .expect_err("--all and a device id are mutually exclusive");

        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::ArgumentConflict,
            "the contradiction must be rejected by the parser, got {error}"
        );
    }

    /// Neither half of the constraint may be broken on its own.
    #[test]
    fn stats_accepts_all_and_a_device_id_separately() {
        let device_id = Uuid::new_v4().to_string();

        parse_args_for_test(["ufa", "devices", "stats", "--all"])
            .expect("--all on its own is valid");
        parse_args_for_test(["ufa", "devices", "stats", &device_id])
            .expect("a device id on its own is valid");
    }

    /// Statistics carrying `uptime` as their only identifying value.
    fn stats_with_uptime(uptime: u64) -> DeviceStatistics {
        DeviceStatistics {
            uptime_sec: Some(uptime),
            last_heartbeat_at: None,
            next_heartbeat_at: None,
            load_average_1min: None,
            load_average_5min: None,
            load_average_15min: None,
            cpu_utilization_pct: None,
            memory_utilization_pct: None,
            uplink: Some(UplinkStatistics {
                tx_rate_bps: None,
                rx_rate_bps: None,
            }),
            interfaces: DeviceInterfaceStatistics { radios: None },
        }
    }

    /// The index a `device-N` test device was built with.
    fn index_of(device: &Device) -> usize {
        device
            .name
            .rsplit('-')
            .next()
            .and_then(|index| index.parse().ok())
            .expect("test devices are named device-N")
    }

    /// One statistics request per device, each an independent round trip, so
    /// they belong in flight together rather than one after another -- but
    /// bounded, because a site can hold hundreds of devices and the controller
    /// answering them is often a home router.
    ///
    /// Concurrency is observed rather than timed: each fake fetch records how
    /// many of its peers are in flight alongside it, and completes after a
    /// number of scheduler yields that decreases with the device index, so the
    /// results necessarily arrive out of device order.
    #[tokio::test]
    async fn device_statistics_are_fetched_concurrently_and_stay_in_device_order() {
        let devices: Vec<Device> = (0..MAX_CONCURRENT_STAT_REQUESTS * 2 + 3)
            .map(|index| test_device(&format!("device-{index}")))
            .collect();
        let in_flight = Cell::new(0_usize);
        let peak_in_flight = Cell::new(0_usize);

        let results = fetch_device_statistics(&devices, |device| {
            let index = index_of(device);
            let yields_before_completing = devices.len() - index;
            let in_flight = &in_flight;
            let peak_in_flight = &peak_in_flight;

            async move {
                in_flight.set(in_flight.get() + 1);
                peak_in_flight.set(peak_in_flight.get().max(in_flight.get()));

                for _ in 0..yields_before_completing {
                    tokio::task::yield_now().await;
                }

                in_flight.set(in_flight.get() - 1);
                Ok(stats_with_uptime(u64::try_from(index).unwrap()))
            }
        })
        .await;

        assert_eq!(
            results.len(),
            devices.len(),
            "every device must be represented in the results"
        );
        for (index, result) in results.iter().enumerate() {
            let stats = result.as_ref().expect("the fake fetch never fails");
            assert_eq!(
                stats.uptime_sec,
                Some(u64::try_from(index).unwrap()),
                "result {index} belongs to a different device: results must stay in device order"
            );
        }

        assert!(
            peak_in_flight.get() > 1,
            "statistics must be fetched concurrently, but only {} request was ever in flight",
            peak_in_flight.get()
        );
        assert!(
            peak_in_flight.get() <= MAX_CONCURRENT_STAT_REQUESTS,
            "concurrency must stay bounded by {MAX_CONCURRENT_STAT_REQUESTS}, saw {} in flight",
            peak_in_flight.get()
        );
    }

    /// The table stays the default rendering.
    #[test]
    fn all_device_stats_default_to_a_table() {
        let fetched = outcomes_for(&["ap-lr"]);

        let report = all_device_stats_report(&fetched, OutputFormat::Table)
            .expect("rendering the all-devices stats must succeed");
        let rendered = document_of(&report);

        assert!(
            rendered.contains(TABLE_CORNER),
            "the default rendering must stay a table, got:\n{rendered}"
        );
        assert!(
            rendered.contains("ap-lr"),
            "the table must name the device, got:\n{rendered}"
        );
    }

    /// The document of a report that must hold one.
    ///
    /// # Panics
    ///
    /// Panics if the report holds no document.
    fn document_of(report: &Report) -> &str {
        report
            .document()
            .unwrap_or_else(|| panic!("the report must hold a document"))
    }

    /// A table of headings and no rows tells a person nothing. The sentence
    /// says it instead, on standard error, so a redirected table stays empty
    /// rather than holding a sentence.
    #[test]
    fn all_device_stats_of_an_empty_site_draw_no_table_and_say_so() {
        let report = all_device_stats_report(&[], OutputFormat::Table)
            .expect("rendering the all-devices stats must succeed");

        assert_eq!(
            report.document(),
            None,
            "a site with no devices has no table to draw"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("No devices")),
            "the note must say the site holds no devices, got {:?}",
            report.notes()
        );
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;
    use crate::test_server::{
        empty_json, json_response, json_response_with_status, TestServer, INTEGRATION_API,
    };

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// The device collection of a site that holds no devices.
    const NO_DEVICES: &str =
        r#"{ "offset": 0, "limit": 200, "count": 0, "totalCount": 0, "data": [] }"#;

    /// A client for the controller a test runs.
    fn client_for(controller: &TestServer) -> UnifiClient {
        UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client")
    }

    /// `devices stats --all --output json` on a site with no devices is still
    /// read by a program. The answer is `[]`, so `jq '.[]'` reads no devices
    /// rather than failing on a sentence, and the sentence goes to standard
    /// error. This runs the command against a controller, so the site with no
    /// devices takes the same path as a real run.
    #[tokio::test]
    async fn stats_for_every_device_of_an_empty_site_answer_json_with_an_empty_array() {
        let controller = TestServer::replying(&json_response(NO_DEVICES)).await;

        let report =
            get_all_device_stats(&client_for(&controller), Uuid::new_v4(), OutputFormat::Json)
                .await
                .expect("the controller answered the device list");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("--output json must answer with a document, got none"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(document).ok(),
            Some(serde_json::json!([])),
            "a site with no devices must answer with an empty array, got:\n{document}"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("No devices")),
            "the note must say the site holds no devices, got {:?}",
            report.notes()
        );
    }

    /// The device that answers its statistics request.
    const HEALTHY_DEVICE_ID: &str = "00000000-0000-0000-0000-00000000000a";

    /// The device whose statistics request fails.
    const FAILING_DEVICE_ID: &str = "00000000-0000-0000-0000-00000000000b";

    /// The reason the controller gives for the failed statistics request.
    const FAILURE_MESSAGE: &str = "Device statistics are not available";

    /// The statistics of the healthy device, in the controller's own shape.
    const HEALTHY_STATISTICS: &str = r#"{
        "uptimeSec": 273900,
        "lastHeartbeatAt": "2026-09-29T12:00:00Z",
        "nextHeartbeatAt": "2026-09-29T12:00:30Z",
        "loadAverage1Min": 0.5,
        "loadAverage5Min": 0.25,
        "loadAverage15Min": 0.125,
        "cpuUtilizationPct": 12.5,
        "memoryUtilizationPct": 40.25,
        "uplink": { "txRateBps": 1200000, "rxRateBps": 3400 },
        "interfaces": { "radios": [ { "frequencyGHz": 5.0, "txRetriesPct": 1.5 } ] }
    }"#;

    /// The device collection of a site that holds the two devices above.
    fn two_devices() -> String {
        format!(
            r#"{{
                "offset": 0, "limit": 200, "count": 2, "totalCount": 2,
                "data": [
                    {{
                        "id": "{HEALTHY_DEVICE_ID}",
                        "name": "ap-lr", "model": "U6-LR",
                        "macAddress": "00:11:22:33:44:55", "ipAddress": "192.168.1.2",
                        "state": "ONLINE", "features": [], "interfaces": []
                    }},
                    {{
                        "id": "{FAILING_DEVICE_ID}",
                        "name": "switch-8", "model": "USW-Lite-8-PoE",
                        "macAddress": "00:11:22:33:44:66", "ipAddress": "192.168.1.3",
                        "state": "OFFLINE", "features": [], "interfaces": []
                    }}
                ]
            }}"#
        )
    }

    /// `devices stats <id> --output json` prints the statistics as the
    /// controller gave them. `devices stats --all --output json` must give the
    /// same figures in the same units, so one parser reads both. A figure such
    /// as `"12.5%"` or `"3d 4h 5m"` is text for a person, and the placeholder
    /// `"ERROR"` in every field hides the reason for the failure.
    #[tokio::test]
    async fn stats_for_every_device_answer_json_with_the_raw_statistics_or_the_error() {
        let controller = TestServer::routing(
            &[
                (
                    format!("/devices/{HEALTHY_DEVICE_ID}/"),
                    json_response(HEALTHY_STATISTICS),
                ),
                (
                    format!("/devices/{FAILING_DEVICE_ID}/"),
                    json_response_with_status(
                        "404 Not Found",
                        &format!(r#"{{ "message": "{FAILURE_MESSAGE}" }}"#),
                    ),
                ),
            ],
            &json_response(&two_devices()),
        )
        .await;

        let report =
            get_all_device_stats(&client_for(&controller), Uuid::new_v4(), OutputFormat::Json)
                .await
                .expect("the controller answered the device list");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("--output json must answer with a document, got none"));
        let parsed: serde_json::Value = serde_json::from_str(document).unwrap_or_else(|error| {
            panic!("--output json must produce JSON ({error}), got:\n{document}")
        });
        let entries = parsed
            .as_array()
            .unwrap_or_else(|| panic!("--output json must produce an array, got:\n{document}"));
        assert_eq!(entries.len(), 2, "every device must appear:\n{document}");

        // The statistics of one device, exactly as `devices stats <id>
        // --output json` prints them.
        let single_device_document = serde_json::to_value(
            serde_json::from_str::<DeviceStatistics>(HEALTHY_STATISTICS)
                .expect("the healthy statistics are valid"),
        )
        .expect("statistics serialize");

        let healthy = &entries[0];
        assert_eq!(
            healthy["statistics"]["cpuUtilizationPct"],
            serde_json::json!(12.5),
            "the CPU figure must be the number the controller gave, got:\n{document}"
        );
        assert_eq!(
            healthy["statistics"], single_device_document,
            "the statistics must be the document of `devices stats <id>`, got:\n{document}"
        );
        assert_eq!(healthy["id"], HEALTHY_DEVICE_ID, "got:\n{document}");
        assert_eq!(healthy["name"], "ap-lr", "got:\n{document}");
        assert_eq!(
            healthy.get("error"),
            None,
            "a device that answered has no error, got:\n{document}"
        );

        let failing = &entries[1];
        assert_eq!(failing["id"], FAILING_DEVICE_ID, "got:\n{document}");
        assert_eq!(failing["name"], "switch-8", "got:\n{document}");
        assert!(
            failing["error"]
                .as_str()
                .is_some_and(|error| error.contains(FAILURE_MESSAGE)),
            "a failed device must carry the reason for the failure, got:\n{document}"
        );
        assert_eq!(
            failing.get("statistics"),
            None,
            "a failed device has no statistics, got:\n{document}"
        );
        assert!(
            !document.contains(FETCH_FAILED),
            "the table placeholder must not reach the JSON document, got:\n{document}"
        );
    }

    /// A restart has no document to give. The sentence that says the
    /// controller accepted it is a note, so it goes to standard error.
    #[tokio::test]
    async fn a_restart_reports_on_standard_error_only() {
        let controller = TestServer::replying(&empty_json()).await;

        let report = restart_device(
            &client_for(&controller),
            Some(Uuid::new_v4()),
            Uuid::new_v4(),
        )
        .await
        .expect("the controller accepted the restart");

        assert_eq!(
            report.document(),
            None,
            "a restart must put nothing on standard output"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("restart initiated")),
            "the note must say the restart started, got {:?}",
            report.notes()
        );
    }

    /// The same rule for a port power cycle.
    #[tokio::test]
    async fn a_port_power_cycle_reports_on_standard_error_only() {
        let controller = TestServer::replying(&empty_json()).await;

        let report = power_cycle_port(
            &client_for(&controller),
            Some(Uuid::new_v4()),
            Uuid::new_v4(),
            3,
        )
        .await
        .expect("the controller accepted the power cycle");

        assert_eq!(
            report.document(),
            None,
            "a power cycle must put nothing on standard output"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("power cycle initiated")),
            "the note must say the power cycle started, got {:?}",
            report.notes()
        );
    }

    /// `devices restart` posts the `RESTART` action to the actions path of
    /// the device the user named, under the site the user named.
    ///
    /// The assertion reads the request the controller got. A wrong path or a
    /// wrong action restarts no device, or the wrong one, and nothing but real
    /// hardware shows it.
    #[tokio::test]
    async fn a_restart_posts_the_restart_action_to_the_device() {
        let controller = TestServer::replying(&empty_json()).await;
        let site_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();

        handle_devices_command(
            DevicesCommand::Restart { device_id },
            Some(site_id),
            &client_for(&controller),
            OutputFormat::Table,
        )
        .await
        .expect("the controller accepted the restart");

        controller.assert_one_json_request(
            &format!("POST {INTEGRATION_API}/sites/{site_id}/devices/{device_id}/actions HTTP/1.1"),
            &serde_json::json!({ "action": "RESTART" }),
        );
    }

    /// `devices power-cycle-port` posts the `POWER_CYCLE` action to the
    /// actions path of the one port the user named. The same action on the
    /// path of the device restarts the whole device.
    #[tokio::test]
    async fn a_port_power_cycle_posts_the_power_cycle_action_to_the_port() {
        const PORT_INDEX: u32 = 7;

        let controller = TestServer::replying(&empty_json()).await;
        let site_id = Uuid::new_v4();
        let device_id = Uuid::new_v4();

        handle_devices_command(
            DevicesCommand::PowerCyclePort {
                device_id,
                port_idx: PORT_INDEX,
            },
            Some(site_id),
            &client_for(&controller),
            OutputFormat::Table,
        )
        .await
        .expect("the controller accepted the power cycle");

        controller.assert_one_json_request(
            &format!(
                "POST {INTEGRATION_API}/sites/{site_id}/devices/{device_id}/interfaces/ports/{PORT_INDEX}/actions HTTP/1.1"
            ),
            &serde_json::json!({ "action": "POWER_CYCLE" }),
        );
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::test_server::{json_response, TestServer};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// How many devices the pretend controller says it holds.
    const TOTAL: &str = "100";

    /// One page of the device collection: a single item, out of a hundred.
    const ONE_DEVICE_OF_A_HUNDRED: &str = r#"{
        "offset": 0, "limit": 1, "count": 1, "totalCount": 100,
        "data": [
            {
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "ap-lr", "model": "U6-LR",
                "macAddress": "00:11:22:33:44:55", "ipAddress": "192.168.1.2",
                "state": "ONLINE", "features": [], "interfaces": []
            }
        ]
    }"#;

    /// A page that holds a single device of a hundred looks exactly like the whole
    /// listing of a controller that has one. The note that tells the two apart
    /// is decided in `output`, and this proves the device listing asks for it: a
    /// command that renders its page on its own answers with no note at all.
    #[tokio::test]
    async fn a_short_page_of_devices_reports_what_it_left_out() {
        let controller = TestServer::replying(&json_response(ONE_DEVICE_OF_A_HUNDRED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let listing = list_devices(&client, Some(Uuid::new_v4()), 1, 0, OutputFormat::Table)
            .await
            .expect("the controller answered the listing");

        let notice = listing
            .notes()
            .first()
            .expect("a page short of the stated total must say so");
        assert!(
            notice.contains(TOTAL),
            "the note must report the total the controller stated, got: {notice}"
        );
        assert!(
            notice.contains("--limit") && notice.contains("--offset"),
            "the note must say how to reach the rest, got: {notice}"
        );
    }
}
