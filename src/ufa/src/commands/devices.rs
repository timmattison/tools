use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    device_helper::get_device_id_or_prompt,
    models::{Device, DeviceAction, DeviceDetails, DeviceStatistics, Page, PortAction},
    output::{print_single_item, print_vec_table, render_vec_table, OutputFormat},
    pagination::fetch_all,
    site_helper::get_site_id_or_prompt,
};

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
            mac_address: device.mac_address.clone(),
            ip_address: device.ip_address.clone(),
            state: format!("{:?}", device.state),
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
        None => "N/A".to_string(),
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
        None => "N/A".to_string(),
    }
}

fn format_percentage(pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{:.1}%", p),
        None => "N/A".to_string(),
    }
}

fn format_load_avg(load: Option<f64>) -> String {
    match load {
        Some(l) => format!("{:.2}", l),
        None => "N/A".to_string(),
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
    fn from_device_and_stats(
        device: &Device,
        stats: Option<&crate::models::DeviceStatistics>,
    ) -> Self {
        match stats {
            Some(s) => Self {
                name: device.name.clone(),
                model: device.model.clone(),
                uptime: format_uptime(s.uptime_sec),
                cpu_pct: format_percentage(s.cpu_utilization_pct),
                memory_pct: format_percentage(s.memory_utilization_pct),
                load_avg_1m: format_load_avg(s.load_average_1min),
                tx_rate: format_rate(s.uplink.as_ref().and_then(|u| u.tx_rate_bps)),
                rx_rate: format_rate(s.uplink.as_ref().and_then(|u| u.rx_rate_bps)),
            },
            None => Self {
                name: device.name.clone(),
                model: device.model.clone(),
                uptime: "N/A".to_string(),
                cpu_pct: "N/A".to_string(),
                memory_pct: "N/A".to_string(),
                load_avg_1m: "N/A".to_string(),
                tx_rate: "N/A".to_string(),
                rx_rate: "N/A".to_string(),
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
            print_single_item(&page, output_format)?;
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

    print_single_item(&device, output_format)?;
    Ok(())
}

async fn get_device_stats(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    device_id: Option<Uuid>,
    all: bool,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;

    // Validate that --all and device_id are mutually exclusive
    if all && device_id.is_some() {
        anyhow::bail!("Cannot specify both --all and a device ID. Use either --all to show all devices or specify a single device ID.");
    }

    if all {
        // Get stats for all devices
        get_all_device_stats(client, site_id, output_format).await
    } else if let Some(device_id) = device_id {
        // Get stats for specific device
        get_single_device_stats(client, site_id, device_id, output_format).await
    } else {
        // Show device selection prompt
        let device_id = get_device_id_or_prompt(client, site_id, device_id).await?;
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
            print_single_item(&stats, output_format)?;
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

    // Create rows with device information and stats
    let mut stats_rows = Vec::new();

    // Fetch statistics for each device (sequentially for now to avoid ownership issues)
    for device in &devices {
        let stats_path = format!("sites/{}/devices/{}/statistics/latest", site_id, device.id);
        let stats = client.get::<DeviceStatistics>(&stats_path).await.ok();
        let row = DeviceStatsRowWithName::from_device_and_stats(device, stats.as_ref());
        stats_rows.push(row);
    }

    println!("{}", render_all_device_stats(&stats_rows, output_format)?);

    Ok(())
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
    let _ = format;
    render_vec_table(rows, OutputFormat::Table)
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
    use crate::models::{DeviceFeature, DeviceInterface, DeviceState};

    /// Box-drawing corner produced by the table renderer.
    const TABLE_CORNER: char = '┌';

    /// A device with just enough shape to build a statistics row from.
    fn test_device(name: &str) -> Device {
        Device {
            id: Uuid::new_v4(),
            name: name.to_string(),
            model: "U6-LR".to_string(),
            mac_address: "00:11:22:33:44:55".to_string(),
            ip_address: "192.168.1.2".to_string(),
            state: DeviceState::Online,
            features: vec![DeviceFeature::AccessPoint],
            interfaces: vec![DeviceInterface::Radios],
        }
    }

    fn rows_for(names: &[&str]) -> Vec<DeviceStatsRowWithName> {
        names
            .iter()
            .map(|name| DeviceStatsRowWithName::from_device_and_stats(&test_device(name), None))
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
