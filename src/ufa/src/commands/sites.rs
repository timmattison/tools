use crate::{
    client::UnifiClient,
    models::{Page, Site},
    output::{render_page_listing, OutputFormat, PageListing},
};
use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;

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
pub struct SiteRow {
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
        SitesCommand::List {
            limit,
            offset,
            filter,
        } => list_sites(client, limit, offset, filter, output_format)
            .await
            .map(PageListing::print),
    }
}

/// List one page of the sites the controller serves.
///
/// # Arguments
///
/// * `client` - The controller client to list the sites with.
/// * `limit` - How many sites to ask for.
/// * `offset` - Where in the collection the page starts.
/// * `filter` - The API's filter expression, if the user gave one.
/// * `output_format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered page, ready to print.
///
/// # Errors
///
/// Returns an error if the request fails or if the answer cannot be rendered.
async fn list_sites(
    client: &UnifiClient,
    limit: u32,
    offset: u64,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<PageListing> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let mut params: Vec<(&str, &dyn std::fmt::Display)> =
        vec![("limit", &limit_str), ("offset", &offset_str)];

    if let Some(f) = &filter {
        params.push(("filter", f));
    }

    let page: Page<Site> = client.get_with_params("sites", &params).await?;

    render_page_listing::<Site, SiteRow>(&page, output_format)
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::test_server::{json_response, TestServer};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// How many sites the pretend controller says it holds.
    const TOTAL: &str = "100";

    /// One page of the site collection: a single item, out of a hundred.
    const ONE_SITE_OF_A_HUNDRED: &str = r#"{
        "offset": 0, "limit": 1, "count": 1, "totalCount": 100,
        "data": [
            {
                "id": "00000000-0000-0000-0000-000000000004",
                "internalReference": "default",
                "name": "Home"
            }
        ]
    }"#;

    /// A page that holds a single site of a hundred looks exactly like the whole
    /// listing of a controller that has one. The note that tells the two apart
    /// is decided in `output`, and this proves the site listing asks for it: a
    /// command that renders its page on its own answers with no note at all.
    #[tokio::test]
    async fn a_short_page_of_sites_reports_what_it_left_out() {
        let controller = TestServer::replying(&json_response(ONE_SITE_OF_A_HUNDRED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let listing = list_sites(&client, 1, 0, None, OutputFormat::Table)
            .await
            .expect("the controller answered the listing");

        let notice = listing
            .notice()
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
