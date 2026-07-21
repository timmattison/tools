use anyhow::Result;
use clap::Subcommand;
use futures::stream::StreamExt;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    device_helper::get_device_id_or_prompt,
    models::{Device, DeviceAction, DeviceDetails, DeviceStatistics, Page, PortAction},
    output::{print_output, print_vec_table, render_vec_table, OutputFormat},
    pagination::fetch_all,
    site_helper::get_site_id_or_prompt,
};

/// Shown in place of a figure the controller did not report.
const NO_DATA: &str = "N/A";

/// Shown in place of every figure of a device whose statistics request
/// failed, so an unreachable device or a permissions problem cannot be
/// mistaken for a device that simply had nothing to report.
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

pub async fn handle_devices_command(
    command: DevicesCommand,
    site_id: Option<Uuid>,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        DevicesCommand::List { limit, offset } => {
            list_devices(client, site_id, limit, offset, output_format).await
        }
        DevicesCommand::Get { device_id } => {
            get_device(client, site_id, device_id, output_format).await
        }
        DevicesCommand::Stats { device_id, all } => {
            get_device_stats(client, site_id, device_id, all, output_format).await
        }
        DevicesCommand::Restart { device_id } => restart_device(client, site_id, device_id).await,
        DevicesCommand::PowerCyclePort {
            device_id,
            port_idx,
        } => power_cycle_port(client, site_id, device_id, port_idx).await,
    }
}

async fn list_devices(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    limit: u32,
    offset: u64,
    output_format: OutputFormat,
) -> Result<()> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let params: Vec<(&str, &dyn std::fmt::Display)> =
        vec![("limit", &limit_str), ("offset", &offset_str)];

    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/devices", site_id);
    let page: Page<Device> = client.get_with_params(&path, &params).await?;

    match output_format {
        OutputFormat::Json => {
            print_output(&page, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<DeviceRow> = page.data.iter().map(DeviceRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
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
        get_all_device_stats(client, site_id, output_format).await
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

async fn get_all_device_stats(
    client: &UnifiClient,
    site_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let devices_path = format!("sites/{}/devices", site_id);
    let devices: Vec<Device> = fetch_all(client, &devices_path).await?;

    if devices.is_empty() {
        println!("No devices found on this site.");
        return Ok(());
    }

    let statistics = fetch_device_statistics(&devices, |device| {
        let stats_path = format!("sites/{}/devices/{}/statistics/latest", site_id, device.id);
        async move { client.get::<DeviceStatistics>(&stats_path).await }
    })
    .await;

    let stats_rows: Vec<DeviceStatsRowWithName> = devices
        .iter()
        .zip(&statistics)
        .map(|(device, stats)| {
            if let Err(error) = stats {
                eprintln!(
                    "Failed to fetch statistics for {} ({}): {error:#}",
                    device.name, device.id
                );
            }
            DeviceStatsRowWithName::from_device_and_stats(device, stats.as_ref())
        })
        .collect();

    println!("{}", render_all_device_stats(&stats_rows, output_format)?);

    Ok(())
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
/// # Arguments
///
/// * `rows` - One row per device, in device order.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered text, without a trailing newline.
///
/// # Errors
///
/// Returns an error if the rows cannot be serialized.
fn render_all_device_stats(
    rows: &[DeviceStatsRowWithName],
    format: OutputFormat,
) -> Result<String> {
    render_vec_table(rows, format)
}

async fn restart_device(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Uuid,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/devices/{}/actions", site_id, device_id);
    let action = DeviceAction::Restart;

    let _: serde_json::Value = client.post(&path, &action).await?;
    println!("Device restart initiated successfully");
    Ok(())
}

async fn power_cycle_port(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Uuid,
    port_idx: u32,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!(
        "sites/{}/devices/{}/interfaces/ports/{}/actions",
        site_id, device_id, port_idx
    );
    let action = PortAction::PowerCycle;

    let _: serde_json::Value = client.post(&path, &action).await?;
    println!("Port power cycle initiated successfully");
    Ok(())
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

    fn rows_for(names: &[&str]) -> Vec<DeviceStatsRowWithName> {
        names
            .iter()
            .map(|name| {
                DeviceStatsRowWithName::from_device_and_stats(
                    &test_device(name),
                    Ok(&stats_with_uptime(0)),
                )
            })
            .collect()
    }

    /// `--output json` is an explicit request from the user, usually because
    /// the output is being piped into something. Answering it with a table
    /// silently breaks that pipeline.
    #[test]
    fn all_device_stats_honour_the_json_output_format() {
        let rows = rows_for(&["ap-lr", "switch-8"]);

        let rendered = render_all_device_stats(&rows, OutputFormat::Json)
            .expect("rendering the all-devices stats must succeed");
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap_or_else(|error| {
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
        let rows = rows_for(&["ap-lr"]);

        let rendered = render_all_device_stats(&rows, OutputFormat::Table)
            .expect("rendering the all-devices stats must succeed");

        assert!(
            rendered.contains(TABLE_CORNER),
            "the default rendering must stay a table, got:\n{rendered}"
        );
        assert!(
            rendered.contains("ap-lr"),
            "the table must name the device, got:\n{rendered}"
        );
    }
}
