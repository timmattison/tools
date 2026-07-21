use anyhow::Result;

use crate::{
    client::UnifiClient,
    models::ApplicationInfo,
    output::{print_output, OutputFormat},
};

pub async fn handle_info_command(client: &UnifiClient, output_format: OutputFormat) -> Result<()> {
    let info: ApplicationInfo = client.get("info").await?;

    print_output(&info, output_format)?;
    Ok(())
}
