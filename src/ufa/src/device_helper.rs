use anyhow::{Context, Result};
use uuid::Uuid;

use crate::{
    client::UnifiClient, commands::devices::DeviceRow, models::Device, output::print_vec_table,
    pagination::fetch_all,
};

/// Get device ID automatically or prompt user to specify one
pub async fn get_device_id_or_prompt(
    client: &UnifiClient,
    site_id: Uuid,
    provided_device_id: Option<Uuid>,
) -> Result<Uuid> {
    // If device ID was provided, use it
    if let Some(device_id) = provided_device_id {
        return Ok(device_id);
    }

    // Otherwise, fetch every device -- a truncated answer would turn "many
    // devices" into a wrong automatic choice -- and decide what to do
    let path = format!("sites/{}/devices", site_id);
    let devices: Vec<Device> = fetch_all(client, &path)
        .await
        .context("Failed to fetch devices for auto-discovery")?;

    match devices.len() {
        0 => {
            anyhow::bail!(
                "No devices found on this site. \
                Make sure there are devices connected to your UniFi network."
            );
        }
        1 => {
            // Single device - use it automatically
            let device = &devices[0];
            eprintln!("Using device: {} ({})", device.name, device.id);
            Ok(device.id)
        }
        _ => {
            // Multiple devices - show them and ask user to choose
            eprintln!("Multiple devices found:");
            eprintln!();

            let rows: Vec<DeviceRow> = devices.iter().map(DeviceRow::from).collect();
            print_vec_table(&rows, crate::output::OutputFormat::Table)?;

            eprintln!();
            anyhow::bail!(
                "Please specify which device to use by providing the device ID as an argument."
            );
        }
    }
}
