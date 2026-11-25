//! The one way a list command reads a collection.
//!
//! A list command shows either one page of a collection or all of it. This
//! module holds that choice and the reads behind it, so the four list
//! commands do not each carry their own copy of the branch.

use anyhow::Result;
use serde::{de::DeserializeOwned, Serialize};
use tabled::Tabled;

use crate::{
    client::UnifiClient,
    models::Page,
    output::{render_page_listing, render_whole_listing, OutputFormat, Report},
    pagination::fetch_all_matching,
};

/// Which part of a collection a list command shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    /// One page: at most `limit` items, from `offset`.
    Page { limit: u32, offset: u64 },
    /// Every item of the collection, read page by page.
    All,
}

impl Selection {
    /// The selection that the flags of a list command name.
    ///
    /// # Arguments
    ///
    /// * `limit` - The value of `--limit`.
    /// * `offset` - The value of `--offset`.
    /// * `all` - Whether the user gave `--all`.
    ///
    /// # Returns
    ///
    /// [`Selection::All`] when `all` is true, and otherwise the page that
    /// `limit` and `offset` name. The command line refuses `--all` beside
    /// `--limit` or `--offset`, so `limit` and `offset` are defaults when
    /// `all` is true.
    pub fn from_flags(limit: u32, offset: u64, all: bool) -> Self {
        if all {
            Self::All
        } else {
            Self::Page { limit, offset }
        }
    }
}

/// The note that says why a whole listing of an empty collection shows
/// nothing.
///
/// # Arguments
///
/// * `items` - The plural name of the items, for example `"sites"`.
/// * `filtered` - Whether the request carried a filter.
///
/// # Returns
///
/// `No {items} found.`, or `No {items} match the filter.` when `filtered`.
fn empty_collection_note(items: &str, filtered: bool) -> String {
    if filtered {
        format!("No {items} match the filter.")
    } else {
        format!("No {items} found.")
    }
}

/// Read the part of the collection at `path` that `selection` names, and
/// render it.
///
/// # Arguments
///
/// * `client` - The controller client to read the collection with.
/// * `path` - Collection path relative to the API root.
/// * `filter` - The API's filter expression, if the user gave one. Each
///   request carries it.
/// * `items` - The plural name of the items of the collection, as a note
///   names them, for example `"sites"`.
/// * `selection` - The part of the collection to show.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered listing. A page carries a note when it leaves items out. A
/// whole collection carries no note, unless it holds no item. Then one note
/// says that nothing was found, or that nothing matches the filter.
///
/// # Errors
///
/// Returns an error if a request fails, if an answer cannot be parsed, or if
/// the listing cannot be rendered.
pub async fn list_collection<T, R>(
    client: &UnifiClient,
    path: &str,
    filter: Option<&str>,
    items: &str,
    selection: Selection,
    format: OutputFormat,
) -> Result<Report>
where
    T: DeserializeOwned + Serialize,
    R: Serialize + Tabled + for<'a> From<&'a T>,
{
    match selection {
        Selection::Page { limit, offset } => {
            let limit = limit.to_string();
            let offset = offset.to_string();
            let mut params: Vec<(&str, &dyn std::fmt::Display)> =
                vec![("limit", &limit), ("offset", &offset)];
            if let Some(expression) = filter.as_ref() {
                params.push(("filter", expression));
            }

            let page: Page<T> = client.get_with_params(path, &params).await?;
            render_page_listing::<T, R>(&page, format)
        }
        Selection::All => {
            let found: Vec<T> = fetch_all_matching(client, path, filter).await?;
            let report = render_whole_listing::<T, R>(&found, format)?;
            if found.is_empty() {
                Ok(report.with_note(empty_collection_note(items, filter.is_some())))
            } else {
                Ok(report)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::sites::SiteRow;
    use crate::models::Site;
    use crate::test_server::{json_response, Answer, TestServer};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// One site, as the API writes it.
    const SITE: &str = r#"{"id":"00000000-0000-0000-0000-000000000004","internalReference":"default","name":"Home"}"#;

    /// `--all` reads each page, and the report holds each item and no note.
    #[tokio::test]
    async fn all_reads_every_page_and_adds_no_note() {
        let first = format!(
            r#"{{"offset":0,"limit":200,"count":2,"totalCount":3,"data":[{SITE},{SITE}]}}"#
        );
        let second =
            format!(r#"{{"offset":2,"limit":200,"count":1,"totalCount":3,"data":[{SITE}]}}"#);
        let routes: Vec<(String, Answer)> = vec![
            ("offset=0".to_string(), json_response(&first).into()),
            ("offset=2".to_string(), json_response(&second).into()),
        ];
        let server = TestServer::routing(&routes, &json_response("{}")).await;
        let client = UnifiClient::new(server.origin(), API_KEY, false).expect("a loopback URL");

        let report = list_collection::<Site, SiteRow>(
            &client,
            "sites",
            None,
            "sites",
            Selection::All,
            OutputFormat::Json,
        )
        .await
        .expect("the controller answered each page");

        assert!(report.notes().is_empty(), "got: {:?}", report.notes());
        let document: Vec<serde_json::Value> =
            serde_json::from_str(report.document().expect("a listing has a document"))
                .expect("the document is a JSON array");
        assert_eq!(document.len(), 3);
    }

    /// A page that leaves items out says so, and the note names `--all`.
    #[tokio::test]
    async fn a_page_says_how_to_see_every_item() {
        let page =
            format!(r#"{{"offset":0,"limit":1,"count":1,"totalCount":100,"data":[{SITE}]}}"#);
        let server = TestServer::replying(&json_response(&page)).await;
        let client = UnifiClient::new(server.origin(), API_KEY, false).expect("a loopback URL");

        let report = list_collection::<Site, SiteRow>(
            &client,
            "sites",
            None,
            "sites",
            Selection::Page {
                limit: 1,
                offset: 0,
            },
            OutputFormat::Table,
        )
        .await
        .expect("the controller answered the page");

        let note = report.notes().first().expect("a short page must say so");
        assert!(note.contains("--all"), "got: {note}");
    }

    /// The flags name a page unless the user gave `--all`.
    #[test]
    fn the_flags_name_a_selection() {
        assert_eq!(Selection::from_flags(25, 0, true), Selection::All);
        assert_eq!(
            Selection::from_flags(5, 10, false),
            Selection::Page {
                limit: 5,
                offset: 10
            }
        );
    }
}
