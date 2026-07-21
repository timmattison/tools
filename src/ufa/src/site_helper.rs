use anyhow::{Context, Result};
use uuid::Uuid;

use crate::{
    client::UnifiClient, commands::sites::SiteRow, models::Site, output::print_vec_table,
    pagination::fetch_all,
};

/// Get site ID automatically or prompt user to specify one
pub async fn get_site_id_or_prompt(
    client: &UnifiClient,
    provided_site_id: Option<Uuid>,
) -> Result<Uuid> {
    // If site ID was provided, use it
    if let Some(site_id) = provided_site_id {
        return Ok(site_id);
    }

    // Otherwise, fetch every site -- a truncated answer would turn "many
    // sites" into a wrong automatic choice -- and decide what to do
    let sites: Vec<Site> = fetch_all(client, "sites")
        .await
        .context("Failed to fetch sites for auto-discovery")?;

    match sites.len() {
        0 => {
            anyhow::bail!(
                "Well, this is awkward... We didn't think it was possible to have zero sites, \
                but here we are. 🤷\n\n\
                You might want to check your UniFi controller setup."
            );
        }
        1 => {
            // Single site - use it automatically
            let site = &sites[0];
            eprintln!("Using site: {} ({})", site.name, site.id);
            Ok(site.id)
        }
        _ => {
            // Multiple sites - show them and ask user to choose
            eprintln!("Multiple sites found:");
            eprintln!();

            let rows: Vec<SiteRow> = sites.iter().map(SiteRow::from).collect();
            print_vec_table(&rows, crate::output::OutputFormat::Table)?;

            eprintln!();
            anyhow::bail!(
                "Please specify which site to use by providing the site ID as an argument."
            );
        }
    }
}
