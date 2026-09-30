use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;
use tabled::{settings::Style, Table, Tabled};

use crate::models::Page;

/// How a user reaches the part of a collection that one page left out.
const PAGING_HINT: &str = "Use --limit and --offset to see the rest.";

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum OutputFormat {
    Json,
    Table,
}

/// Render `data` in the requested output format and return it as a string.
///
/// This is the pure counterpart of [`print_output`]: it performs all of the
/// formatting work and never touches stdout, so it can be unit tested
/// directly.
///
/// # Arguments
///
/// * `data` - Any serializable value.
/// * `format` - The output format to render.
///
/// # Returns
///
/// The rendered text, without a trailing newline.
///
/// # Errors
///
/// Returns an error if `data` cannot be serialized.
pub fn render_output<T>(data: &T, format: OutputFormat) -> Result<String>
where
    T: Serialize + ?Sized,
{
    match format {
        OutputFormat::Json => Ok(serde_json::to_string_pretty(data)?),
        OutputFormat::Table => Ok(render_key_value_table(&serde_json::to_value(data)?)),
    }
}

/// A single `Field` / `Value` line of a key/value table.
#[derive(Tabled)]
struct KeyValueRow {
    #[tabled(rename = "Field")]
    field: String,
    #[tabled(rename = "Value")]
    value: String,
}

/// Render an arbitrary JSON value as a two-column key/value table.
///
/// Nested objects are flattened into dotted keys (`features.switching.enabled`)
/// and array elements are indexed (`uplinks[0]`), so an arbitrarily deep API
/// response still renders as a flat, scannable list of fields.
///
/// # Arguments
///
/// * `value` - The JSON representation of the item to render.
///
/// # Returns
///
/// The rendered table, without a trailing newline.
fn render_key_value_table(value: &serde_json::Value) -> String {
    // `flatten_into_rows` always emits at least one row: an empty object or
    // array at the root is itself treated as a leaf.
    let mut rows = Vec::new();
    flatten_into_rows(String::new(), value, &mut rows);

    let mut table = Table::new(rows);
    table.with(Style::modern());
    table.to_string()
}

/// Recursively flatten `value` into `rows`, prefixing each key with `prefix`.
///
/// # Arguments
///
/// * `prefix` - Dotted/indexed path accumulated so far; empty at the root.
/// * `value` - The JSON value to flatten.
/// * `rows` - Accumulator the flattened rows are appended to.
fn flatten_into_rows(prefix: String, value: &serde_json::Value, rows: &mut Vec<KeyValueRow>) {
    match value {
        serde_json::Value::Object(map) if !map.is_empty() => {
            for (key, child) in map {
                let child_prefix = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten_into_rows(child_prefix, child, rows);
            }
        }
        serde_json::Value::Array(items) if !items.is_empty() => {
            for (index, child) in items.iter().enumerate() {
                flatten_into_rows(format!("{prefix}[{index}]"), child, rows);
            }
        }
        leaf => rows.push(KeyValueRow {
            field: if prefix.is_empty() {
                "value".to_string()
            } else {
                prefix
            },
            value: leaf_to_string(leaf),
        }),
    }
}

/// Render a JSON leaf (or an empty container) as a display string.
///
/// Strings are rendered without their surrounding quotes so the table reads
/// naturally; everything else keeps its JSON spelling (`null`, `true`, `12`,
/// `[]`, `{}`).
///
/// # Arguments
///
/// * `value` - The leaf value to render.
///
/// # Returns
///
/// The display string for `value`.
fn leaf_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[allow(
    clippy::print_stdout,
    reason = "this prints the document the user asked for, which is what standard output carries"
)]
pub fn print_output<T>(data: &T, format: OutputFormat) -> Result<()>
where
    T: Serialize + ?Sized,
{
    println!("{}", render_output(data, format)?);
    Ok(())
}

/// Render `data` as a multi-column table, one row per item.
///
/// # Arguments
///
/// * `data` - The rows to render.
///
/// # Returns
///
/// The rendered table, without a trailing newline.
pub fn render_table<T>(data: &[T]) -> String
where
    T: Tabled,
{
    let mut table = Table::new(data);
    table.with(Style::modern());
    table.to_string()
}

/// Render a list of items in the requested output format and return it as a
/// string.
///
/// This is the pure counterpart of [`print_vec_table`], so command output can
/// be asserted on directly in tests.
///
/// # Arguments
///
/// * `data` - The rows to render.
/// * `format` - The output format to render.
///
/// # Returns
///
/// The rendered text, without a trailing newline.
///
/// # Errors
///
/// Returns an error if `data` cannot be serialized as JSON.
pub fn render_vec_table<T>(data: &[T], format: OutputFormat) -> Result<String>
where
    T: Serialize + Tabled,
{
    match format {
        OutputFormat::Json => render_output(data, format),
        OutputFormat::Table => Ok(render_table(data)),
    }
}

// Specific implementation for vectors with Tabled items
#[allow(
    clippy::print_stdout,
    reason = "this prints the document the user asked for, which is what standard output carries"
)]
pub fn print_vec_table<T>(data: &[T], format: OutputFormat) -> Result<()>
where
    T: Serialize + Tabled,
{
    println!("{}", render_vec_table(data, format)?);
    Ok(())
}

/// What a list command shows: one page of a collection, and the note that
/// says the page is not all of it.
///
/// A list command asks for a single page on purpose -- `--limit` and
/// `--offset` are its arguments -- so what comes back is often a slice of
/// something larger. A table of 25 devices from a site of a hundred looks
/// exactly like a table of every device on a site of 25. The note is the only
/// thing that tells the two apart, and it reads the same whichever collection
/// was listed, so it is decided here instead of in each of the four commands
/// that list one.
#[must_use = "a listing that is never printed tells the user nothing"]
pub struct PageListing {
    /// The page itself, in the format the user asked for.
    body: String,
    /// What the page left out, when it left anything out.
    notice: Option<String>,
}

impl PageListing {
    /// What this listing says about the items it left out.
    ///
    /// # Returns
    ///
    /// The note, or `None` when the page holds the whole collection.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Print the listing: the page on standard output, the note on standard
    /// error.
    ///
    /// `--output json` is asked for by somebody who reads the answer with a
    /// program, and a sentence on standard output is a line no such program
    /// parses. Standard error carries the note in both formats rather than in
    /// one, so there is a single rule about where a note goes, and it is the
    /// stream this crate already reports on -- the site it picked for the
    /// user, say.
    #[allow(
        clippy::print_stdout,
        reason = "the page is the document the user asked for; the note beside it goes to standard error"
    )]
    pub fn print(self) {
        println!("{}", self.body);

        // After the page, not before it: the note is what the user reads last.
        if let Some(notice) = self.notice() {
            eprintln!("{notice}");
        }
    }
}

/// Render one page of a collection for a list command.
///
/// # Arguments
///
/// * `page` - The page the controller answered with.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered page, together with the note about whatever it left out.
///
/// # Errors
///
/// Returns an error if the page cannot be serialized as JSON.
pub fn render_page_listing<T, R>(page: &Page<T>, format: OutputFormat) -> Result<PageListing>
where
    T: Serialize,
    R: Serialize + Tabled + for<'a> From<&'a T>,
{
    let body = match format {
        // The whole page, `totalCount` included: a program that reads this
        // wants the figure itself rather than a sentence about it.
        OutputFormat::Json => render_output(page, format)?,
        OutputFormat::Table => {
            let rows: Vec<R> = page.data.iter().map(R::from).collect();
            render_table(&rows)
        }
    };

    Ok(PageListing {
        body,
        notice: truncation_notice(page.data.len(), page.total_count, page.offset),
    })
}

/// The line that says a page is not the whole collection.
///
/// # Arguments
///
/// * `shown` - How many items this page holds.
/// * `total` - How many items the collection holds, as the server counted
///   them.
/// * `offset` - Where in the collection this page starts.
///
/// # Returns
///
/// The note, or `None` when the page holds every item there is. A note on a
/// complete listing is noise, and a note the user learns to skip stops being
/// a signal on the listing that needs one.
fn truncation_notice(shown: usize, total: u64, offset: u64) -> Option<String> {
    let shown = u64::try_from(shown).unwrap_or(u64::MAX);

    // The page holds every item the server counted, so there is nothing to
    // report. This also covers the collection that is empty: nothing of
    // nothing is still all of it.
    if shown >= total {
        return None;
    }

    // An offset past the end of the collection answers with no items at all.
    // There is no range to name, and the count is the whole point: a table of
    // headings and nothing else otherwise reads as a controller with nothing
    // on it.
    if shown == 0 {
        return Some(format!(
            "This page shows none of the {total} items. {PAGING_HINT}"
        ));
    }

    let first = offset.saturating_add(1);
    let last = offset.saturating_add(shown);

    Some(format!(
        "This page shows {first}-{last} of {total}. {PAGING_HINT}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Box-drawing corner produced by `Style::modern()`.
    const TABLE_CORNER: char = '┌';

    #[test]
    fn test_table_format_is_not_json() {
        let data = json!({ "name": "ap-lr", "state": "ONLINE" });

        let table = render_output(&data, OutputFormat::Table).unwrap();
        let json = render_output(&data, OutputFormat::Json).unwrap();

        assert_ne!(
            table, json,
            "--output table must not fall through to the JSON renderer"
        );
        assert!(
            table.contains(TABLE_CORNER),
            "table output should be drawn with box-drawing characters, got:\n{table}"
        );
        assert!(
            !table.contains("\"name\""),
            "table output should not contain JSON-quoted keys, got:\n{table}"
        );
    }

    #[test]
    fn test_table_format_renders_field_and_value_rows() {
        let data = json!({ "name": "ap-lr", "port_count": 8, "adopted": true });

        let table = render_output(&data, OutputFormat::Table).unwrap();

        assert!(table.contains("Field"), "missing Field header:\n{table}");
        assert!(table.contains("Value"), "missing Value header:\n{table}");
        assert!(table.contains("name"), "missing name row:\n{table}");
        assert!(table.contains("ap-lr"), "missing name value:\n{table}");
        assert!(table.contains("port_count"), "missing number row:\n{table}");
        assert!(table.contains('8'), "missing number value:\n{table}");
        assert!(table.contains("adopted"), "missing bool row:\n{table}");
        assert!(table.contains("true"), "missing bool value:\n{table}");
    }

    #[test]
    fn test_table_format_flattens_nested_objects_with_dotted_keys() {
        let data = json!({ "features": { "switching": { "enabled": true } } });

        let table = render_output(&data, OutputFormat::Table).unwrap();

        assert!(
            table.contains("features.switching.enabled"),
            "nested keys should be flattened with dots, got:\n{table}"
        );
    }

    #[test]
    fn test_table_format_renders_array_elements_with_indices() {
        let data = json!({ "uplinks": ["eth0", "eth1"] });

        let table = render_output(&data, OutputFormat::Table).unwrap();

        assert!(
            table.contains("uplinks[0]"),
            "array elements should be indexed, got:\n{table}"
        );
        assert!(table.contains("eth1"), "missing second element:\n{table}");
    }

    #[test]
    fn test_table_format_renders_a_struct() {
        #[derive(Serialize)]
        struct Info {
            application_version: String,
        }

        let table = render_output(
            &Info {
                application_version: "9.0.114".to_string(),
            },
            OutputFormat::Table,
        )
        .unwrap();

        assert!(table.contains(TABLE_CORNER), "not a table:\n{table}");
        assert!(
            table.contains("application_version"),
            "missing field label:\n{table}"
        );
        assert!(table.contains("9.0.114"), "missing field value:\n{table}");
    }

    #[test]
    fn test_json_format_round_trips() {
        let data = json!({ "name": "ap-lr", "uplinks": ["eth0"], "adopted": true });

        let rendered = render_output(&data, OutputFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap();

        assert_eq!(parsed, data);
    }

    /// An item of a stand-in collection, so a listing can be rendered without
    /// a controller to list.
    #[derive(Serialize)]
    struct Item {
        name: String,
    }

    /// How one stand-in item is shown in a table.
    #[derive(Serialize, Tabled)]
    struct ItemRow {
        #[tabled(rename = "Name")]
        name: String,
    }

    impl From<&Item> for ItemRow {
        fn from(item: &Item) -> Self {
            Self {
                name: item.name.clone(),
            }
        }
    }

    /// One page of a collection: `shown` items, taken from `offset`, out of a
    /// collection the server counted `total` of.
    fn page_of(shown: usize, total: u64, offset: u64) -> Page<Item> {
        let data: Vec<Item> = (0..shown)
            .map(|index| Item {
                name: format!("item-{index}"),
            })
            .collect();
        let count = u32::try_from(data.len()).expect("a test page holds few items");

        Page {
            offset,
            limit: count,
            count,
            total_count: total,
            data,
        }
    }

    /// A listing that holds everything there is has nothing to report. A note
    /// on every listing is noise, and a note a user learns to skip stops being
    /// a signal on the listing that needs one.
    #[test]
    fn a_page_that_holds_the_whole_collection_says_nothing() {
        assert_eq!(
            truncation_notice(3, 3, 0),
            None,
            "a listing of everything must not claim to have left anything out"
        );
    }

    /// The same, for the collection that is exactly one page long. This is
    /// the case a rule written as "the page is full, so ask again" gets wrong.
    #[test]
    fn a_page_that_exactly_fills_the_stated_total_says_nothing() {
        assert_eq!(
            truncation_notice(25, 25, 0),
            None,
            "a full page that is also the whole collection has left nothing out"
        );
    }

    /// A controller with nothing to list is complete, not truncated.
    #[test]
    fn an_empty_collection_says_nothing() {
        assert_eq!(
            truncation_notice(0, 0, 0),
            None,
            "an empty collection is shown in full"
        );
    }

    /// The default run: 25 of a hundred, and no sign of the other 75.
    #[test]
    fn a_first_page_short_of_the_total_names_both_figures() {
        let notice = truncation_notice(25, 100, 0)
            .expect("a page that shows a quarter of the collection must say so");

        assert!(
            notice.contains("1-25"),
            "the note must say which items the page holds, got: {notice}"
        );
        assert!(
            notice.contains("100"),
            "the note must report the total the server stated, got: {notice}"
        );
        assert!(
            notice.contains("--limit") && notice.contains("--offset"),
            "the note must say how to reach the rest, got: {notice}"
        );
    }

    /// A page reached with `--offset` holds items that start where the offset
    /// says, so a note that counts from one describes a different page.
    #[test]
    fn a_page_reached_with_an_offset_counts_from_that_offset() {
        let notice = truncation_notice(25, 100, 25)
            .expect("the second page of four must say what it left out");

        assert!(
            notice.contains("26-50"),
            "the note must count from the offset the page came from, got: {notice}"
        );
        assert!(
            notice.contains("100"),
            "the note must report the total the server stated, got: {notice}"
        );
    }

    /// An offset past the end of the collection answers with no items at all,
    /// and a hundred items the user cannot see. That is the listing that most
    /// needs a note.
    #[test]
    fn a_page_past_the_end_of_the_collection_still_reports_the_total() {
        let notice = truncation_notice(0, 100, 500)
            .expect("a page beyond the collection must not read as an empty controller");

        assert!(
            notice.contains("100"),
            "the note must report the total the server stated, got: {notice}"
        );
        assert!(
            !notice.contains("501"),
            "a page that holds no items has no range to name, got: {notice}"
        );
    }

    /// `--output json` is read by a program. The note is a sentence, so it
    /// goes nowhere near the document.
    #[test]
    fn a_json_listing_keeps_the_note_out_of_the_document() {
        let listing = render_page_listing::<Item, ItemRow>(&page_of(2, 100, 0), OutputFormat::Json)
            .expect("rendering a page must succeed");

        let parsed: serde_json::Value =
            serde_json::from_str(&listing.body).unwrap_or_else(|error| {
                panic!(
                    "--output json must produce JSON ({error}), got:\n{}",
                    listing.body
                )
            });

        assert_eq!(
            parsed["totalCount"], 100,
            "the document must carry the figure itself, got:\n{}",
            listing.body
        );
        assert!(
            listing.notice().is_some(),
            "two items of a hundred must still be reported as a partial listing"
        );
    }

    /// The table is the body, and the note is not part of it: a run that
    /// redirects the table to a file must not find a sentence in the middle of
    /// its rows.
    #[test]
    fn a_table_listing_keeps_the_note_out_of_the_table() {
        let listing =
            render_page_listing::<Item, ItemRow>(&page_of(2, 100, 0), OutputFormat::Table)
                .expect("rendering a page must succeed");

        assert!(
            listing.body.contains(TABLE_CORNER),
            "the body must stay a table, got:\n{}",
            listing.body
        );
        assert!(
            !listing.body.contains("--limit"),
            "the note belongs beside the table, not in it, got:\n{}",
            listing.body
        );
        assert!(
            listing.notice().is_some(),
            "two items of a hundred must be reported as a partial listing"
        );
    }
}
