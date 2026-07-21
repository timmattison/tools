use anyhow::Result;
use clap::ValueEnum;
use serde::Serialize;
use tabled::{settings::Style, Table, Tabled};

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
pub fn print_vec_table<T>(data: &[T], format: OutputFormat) -> Result<()>
where
    T: Serialize + Tabled,
{
    println!("{}", render_vec_table(data, format)?);
    Ok(())
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
}
