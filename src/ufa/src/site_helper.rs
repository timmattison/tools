use anyhow::Result;
use uuid::Uuid;

use crate::{
    chooser::{choose_id, Choosable},
    client::UnifiClient,
    commands::sites::SiteRow,
    models::Site,
};

impl Choosable for Site {
    type Row = SiteRow;

    const NOUN: &'static str = "site";
    const PLURAL: &'static str = "sites";
    const NONE_FOUND: &'static str = "Well, this is awkward... We didn't think it was possible to \
        have zero sites, but here we are. 🤷\n\n\
        You might want to check your UniFi controller setup.";
    const HOW_TO_SPECIFY: &'static str = "Several sites exist and there is no terminal to ask at. \
         Pass --site-id to say which site to use.";

    fn id(&self) -> Uuid {
        self.id
    }

    fn label(&self) -> &str {
        &self.name
    }
}

/// Get the site ID to work with, asking the user when it is ambiguous.
///
/// A site id given on the command line is used as-is. Otherwise the
/// controller's sites are listed: a single site is used automatically, and
/// several are shown as a table the user picks from. When there is no
/// terminal to ask at — a piped or scripted run — the choice has to be named
/// with `--site-id` instead.
///
/// # Arguments
///
/// * `client` - The controller client to list sites with.
/// * `provided_site_id` - The site id the user named, if any.
///
/// # Returns
///
/// The id of the site to operate on.
///
/// # Errors
///
/// Returns an error if the sites cannot be listed, if there are none, or if
/// the choice is ambiguous and cannot be put to anyone.
pub async fn get_site_id_or_prompt(
    client: &UnifiClient,
    provided_site_id: Option<Uuid>,
) -> Result<Uuid> {
    choose_id::<Site>(client, "sites", provided_site_id).await
}
