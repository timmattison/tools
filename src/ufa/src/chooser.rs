//! Turning "which one did you mean?" into an id.
//!
//! Nearly every command needs a site id, and the device commands need a
//! device id on top of that. The rules are the same in both cases and are the
//! interesting part: fetch the whole collection (a truncated answer would
//! turn "many" into a wrong automatic choice), use the only one if there is
//! only one, show the choices and ask when there are several, and say
//! something useful when there are none or when there is nobody to ask.
//!
//! Those rules live here once. A collection joins in by describing itself
//! through [`Choosable`] — its noun, its row type, and what to say when the
//! user has to name one by hand.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    output::{print_vec_table, OutputFormat},
    pagination::fetch_all,
    prompt::{self, Console},
};

/// A collection the CLI can pick a single item out of.
pub trait Choosable: DeserializeOwned {
    /// How one of these is shown in the table of choices.
    type Row: Tabled + Serialize + for<'a> From<&'a Self>;

    /// What one of these is called, e.g. `site`.
    const NOUN: &'static str;

    /// What several of these are called, e.g. `sites`.
    const PLURAL: &'static str;

    /// What to say when the controller has none at all.
    const NONE_FOUND: &'static str;

    /// How the user names one explicitly, for when nobody can be asked.
    const HOW_TO_SPECIFY: &'static str;

    /// The id that identifies this item to the API.
    fn id(&self) -> Uuid;

    /// A short human name, used when reporting an automatic choice.
    fn label(&self) -> &str;
}

/// Decide which of `items` to use, asking the user if it takes asking.
///
/// # Arguments
///
/// * `items` - Everything the controller has of this kind.
/// * `console` - Where the question is put, if one is needed.
///
/// # Returns
///
/// The id of the chosen item.
///
/// # Errors
///
/// Returns an error if there are none to choose from, or if there are several
/// and no terminal to ask at.
fn choose_from<T: Choosable>(items: &[T], console: &mut impl Console) -> Result<Uuid> {
    match items {
        [] => anyhow::bail!("{}", T::NONE_FOUND),
        [only] => {
            eprintln!("Using {}: {} ({})", T::NOUN, only.label(), only.id());
            Ok(only.id())
        }
        many => {
            eprintln!("Multiple {} found:", T::PLURAL);
            eprintln!();

            let rows: Vec<T::Row> = many.iter().map(T::Row::from).collect();
            print_vec_table(&rows, OutputFormat::Table)?;
            eprintln!();

            match prompt::select_one(console, &format!("Select a {}", T::NOUN), many.len())? {
                Some(index) => Ok(many[index].id()),
                // No terminal to ask at: the choice has to come from the
                // command line instead.
                None => anyhow::bail!("{}", T::HOW_TO_SPECIFY),
            }
        }
    }
}

/// Resolve the id of the `T` to work with.
///
/// # Arguments
///
/// * `client` - The controller client to list the collection with.
/// * `path` - Collection path relative to the API root.
/// * `provided` - The id the user already named, if any.
///
/// # Returns
///
/// The provided id, or the one that was chosen for or by the user.
///
/// # Errors
///
/// Returns an error if the collection cannot be listed, if it is empty, or if
/// the choice cannot be made.
pub async fn choose_id<T: Choosable>(
    client: &UnifiClient,
    path: &str,
    provided: Option<Uuid>,
) -> Result<Uuid> {
    if let Some(id) = provided {
        return Ok(id);
    }

    // Every item, not the first page: a truncated answer would turn "several
    // of them" into a wrong automatic choice.
    let items: Vec<T> = fetch_all(client, path)
        .await
        .with_context(|| format!("Failed to fetch {} for auto-discovery", T::PLURAL))?;

    choose_from(&items, &mut prompt::Stdio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::Scripted;
    use serde::Deserialize;

    /// A stand-in collection, so the choosing rules can be exercised without
    /// a controller to list.
    #[derive(Debug, Deserialize)]
    struct Thing {
        id: Uuid,
        name: String,
    }

    #[derive(Tabled, Serialize)]
    struct ThingRow {
        #[tabled(rename = "ID")]
        id: String,
        #[tabled(rename = "Name")]
        name: String,
    }

    impl From<&Thing> for ThingRow {
        fn from(thing: &Thing) -> Self {
            Self {
                id: thing.id.to_string(),
                name: thing.name.clone(),
            }
        }
    }

    impl Choosable for Thing {
        type Row = ThingRow;
        const NOUN: &'static str = "thing";
        const PLURAL: &'static str = "things";
        const NONE_FOUND: &'static str = "No things found on this controller.";
        const HOW_TO_SPECIFY: &'static str = "Pass --thing-id to say which one to use.";

        fn id(&self) -> Uuid {
            self.id
        }

        fn label(&self) -> &str {
            &self.name
        }
    }

    /// `count` things, with predictable ids.
    fn things(count: usize) -> Vec<Thing> {
        (0..count)
            .map(|index| Thing {
                id: Uuid::from_u128(index as u128 + 1),
                name: format!("thing {index}"),
            })
            .collect()
    }

    /// The whole point: when there are several, ask, and use the answer.
    #[test]
    fn several_things_are_offered_to_the_user() {
        let items = things(3);
        let mut console = Scripted::terminal(&["2"]);

        let chosen = choose_from(&items, &mut console).expect("the user named a thing");

        assert_eq!(chosen, items[1].id(), "the second thing was chosen");
        assert!(console.was_asked(), "the user must have been asked");
    }

    /// A pipe cannot answer, so it gets told how to name one instead.
    #[test]
    fn a_pipe_is_told_how_to_name_one() {
        let items = things(3);
        let mut console = Scripted::not_a_terminal();

        let error = choose_from(&items, &mut console)
            .expect_err("an ambiguous choice cannot be made without asking");

        assert!(
            format!("{error:#}").contains(Thing::HOW_TO_SPECIFY),
            "the failure must say how to name one, got {error:#}"
        );
        assert!(!console.was_asked(), "there is no terminal to ask at");
    }

    /// One candidate is not a choice.
    #[test]
    fn a_single_thing_is_used_without_asking() {
        let items = things(1);
        let mut console = Scripted::terminal(&[]);

        let chosen = choose_from(&items, &mut console).expect("a single thing needs no choosing");

        assert_eq!(chosen, items[0].id());
        assert!(!console.was_asked(), "there is nothing to ask about");
    }

    /// Nothing to choose from is a problem the user has to hear about.
    #[test]
    fn an_empty_collection_is_reported() {
        let mut console = Scripted::terminal(&[]);

        let error = choose_from::<Thing>(&[], &mut console)
            .expect_err("there is no id to answer with when there is nothing");

        assert!(
            format!("{error:#}").contains(Thing::NONE_FOUND),
            "the failure must explain the empty collection, got {error:#}"
        );
    }
}
