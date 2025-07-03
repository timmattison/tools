use anyhow::Result;

use crate::{client::UnifiClient, models::ApplicationInfo, output::{OutputFormat, print_single_item}};

pub async fn handle_info_command(
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    let info: ApplicationInfo = client.get("/info").await?;
    
    print_single_item(&info, output_format)?;
    Ok(())
}