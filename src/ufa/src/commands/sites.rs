use crate::{
    client::UnifiClient,
    listing::{list_collection, Selection},
    models::Site,
    output::{OutputFormat, Report},
};
use anyhow::Result;
use clap::Args;
use tabled::Tabled;

/// The arguments of `ufa sites`. `Commands::Sites` in `main.rs` flattens this
/// struct, so the four arguments, their defaults and their help text have one
/// copy.
#[derive(Args, Debug, Clone)]
pub struct SitesArgs {
    /// Maximum number of sites to return
    #[clap(long, default_value = "25")]
    limit: u32,

    /// Offset for pagination
    #[clap(long, default_value = "0")]
    offset: u64,

    /// Filter expression
    #[clap(long)]
    filter: Option<String>,

    /// Return every site: read all the pages, not one page
    #[clap(long, conflicts_with_all = ["limit", "offset"])]
    all: bool,
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
    args: SitesArgs,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    let SitesArgs {
        limit,
        offset,
        filter,
        all,
    } = args;

    list_sites(
        client,
        Selection::from_flags(limit, offset, all),
        filter,
        output_format,
    )
    .await
    .map(Report::print)
}

/// List the sites the controller serves: one page, or every site.
///
/// # Arguments
///
/// * `client` - The controller client to list the sites with.
/// * `selection` - One page, or every site.
/// * `filter` - The API's filter expression, if the user gave one.
/// * `output_format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered listing, ready to print.
///
/// # Errors
///
/// Returns an error if a request fails or if the answer cannot be rendered.
async fn list_sites(
    client: &UnifiClient,
    selection: Selection,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<Report> {
    list_collection::<Site, SiteRow>(
        client,
        "sites",
        filter.as_deref(),
        "sites",
        selection,
        output_format,
    )
    .await
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

        let listing = list_sites(
            &client,
            Selection::Page {
                limit: 1,
                offset: 0,
            },
            None,
            OutputFormat::Table,
        )
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

    /// A collection that holds nothing, as the API writes it.
    const EMPTY_COLLECTION: &str = r#"{"offset":0,"limit":200,"count":0,"totalCount":0,"data":[]}"#;

    /// `--all` on a controller with no site must say that it found none.
    #[tokio::test]
    async fn an_empty_site_listing_with_all_says_nothing_was_found() {
        let controller = TestServer::replying(&json_response(EMPTY_COLLECTION)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let listing = list_sites(&client, Selection::All, None, OutputFormat::Table)
            .await
            .expect("the controller answered the listing");

        assert_eq!(listing.document(), None, "an empty table has no document");
        assert_eq!(listing.notes(), ["No sites found."]);
    }
}
