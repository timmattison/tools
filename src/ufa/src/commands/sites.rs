use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;
use crate::{client::UnifiClient, models::{Page, Site}, output::{OutputFormat, print_vec_table, print_single_item}};

#[derive(Subcommand, Debug)]
pub enum SitesCommand {
    /// List all sites
    List {
        /// Maximum number of sites to return
        #[clap(long, default_value = "25")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },
}

#[derive(Tabled, serde::Serialize)]
struct SiteRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Internal Reference")]
    internal_reference: String,
}

impl From<&Site> for SiteRow {
    fn from(site: &Site) -> Self {
        Self {
            id: site.id.to_string(),
            name: site.name.clone(),
            internal_reference: site.internal_reference.clone(),
        }
    }
}

pub async fn handle_sites_command(
    command: SitesCommand,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        SitesCommand::List { limit, offset, filter } => {
            list_sites(client, limit, offset, filter, output_format).await
        }
    }
}

async fn list_sites(
    client: &UnifiClient,
    limit: u32,
    offset: u64,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let mut params: Vec<(&str, &dyn std::fmt::Display)> = vec![
        ("limit", &limit_str),
        ("offset", &offset_str),
    ];

    if let Some(f) = &filter {
        params.push(("filter", f));
    }

    let page: Page<Site> = client.get_with_params("/sites", &params).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&page, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<SiteRow> = page.data.iter().map(SiteRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}