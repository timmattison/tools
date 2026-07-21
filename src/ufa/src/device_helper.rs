use anyhow::Result;
use uuid::Uuid;

use crate::{
    chooser::{choose_id, Choosable},
    client::UnifiClient,
    commands::devices::DeviceRow,
    models::Device,
};

impl Choosable for Device {
    type Row = DeviceRow;

    const NOUN: &'static str = "device";
    const PLURAL: &'static str = "devices";
    const NONE_FOUND: &'static str = "No devices found on this site. \
        Make sure there are devices connected to your UniFi network.";
    const HOW_TO_SPECIFY: &'static str =
        "Several devices exist on this site and there is no terminal to ask at. \
         Name the device id on the command line to say which device to use.";

    fn id(&self) -> Uuid {
        self.id
    }

    fn label(&self) -> &str {
        &self.name
    }
}

/// Get the device ID to work with, asking the user when it is ambiguous.
///
/// A device id given on the command line is used as-is. Otherwise the site's
/// devices are listed: a single device is used automatically, and several are
/// shown as a table the user picks from. When there is no terminal to ask at
/// — a piped or scripted run — the device id has to be named on the command
/// line instead.
///
/// # Arguments
///
/// * `client` - The controller client to list devices with.
/// * `site_id` - The site whose devices are offered.
/// * `provided_device_id` - The device id the user named, if any.
///
/// # Returns
///
/// The id of the device to operate on.
///
/// # Errors
///
/// Returns an error if the devices cannot be listed, if there are none, or if
/// the choice is ambiguous and cannot be put to anyone.
pub async fn get_device_id_or_prompt(
    client: &UnifiClient,
    site_id: Uuid,
    provided_device_id: Option<Uuid>,
) -> Result<Uuid> {
    let path = format!("sites/{site_id}/devices");
    choose_id::<Device>(client, &path, provided_device_id).await
}
