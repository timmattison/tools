//! A guard that reads the documented answers of the controller API into the models.
//!
//! The test builds a JSON document from each answer schema of
//! `integration.json` and deserializes it into the model that reads that
//! answer. A model that refuses a document the spec allows is a defect. A
//! hand-written fixture repeats the assumption of its author, so only a
//! document built from the spec can disagree with the model.
//!
//! Known limit: the client schemas use `allOf` and discriminators. The
//! generator refuses those constructs, so the client models are not covered.

use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use crate::models;

/// The text of the controller API spec.
const SPEC_TEXT: &str = include_str!("../integration.json");

/// The prefix of a `$ref` that points into the schemas of the spec.
const SCHEMA_REF_PREFIX: &str = "#/components/schemas/";

/// The deepest schema nesting the generator follows.
const MAX_DEPTH: usize = 16;

/// Which properties a generated document holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Document {
    /// Only the properties that the `required` list names, at each level.
    Minimal,
    /// Every property, at each level.
    Full,
}

/// Parses the spec.
///
/// # Returns
///
/// The root of the spec as a JSON value.
///
/// # Panics
///
/// Panics when the spec text is not JSON.
fn load_spec() -> Value {
    serde_json::from_str(SPEC_TEXT).expect("integration.json is JSON")
}

/// Builds a JSON document from one schema node.
///
/// # Arguments
///
/// * `root` - The spec root, used to follow `$ref` links.
/// * `node` - The schema node to build from.
/// * `kind` - Which properties the document holds.
/// * `path` - The place of `node` in the spec, for messages.
/// * `depth` - The nesting depth so far.
///
/// # Returns
///
/// A document that the schema allows.
///
/// # Panics
///
/// Panics on a construct the generator does not support, on a `$ref` that
/// does not resolve, and on nesting deeper than [`MAX_DEPTH`]. A refusal is
/// the safe answer: a guard that skips what it cannot read reports clean.
fn generate(root: &Value, node: &Value, kind: Document, path: &str, depth: usize) -> Value {
    assert!(depth <= MAX_DEPTH, "schema nesting is too deep at {path}");
    if let Some(reference) = node.get("$ref") {
        let reference = reference
            .as_str()
            .unwrap_or_else(|| panic!("$ref is not a string at {path}"));
        let name = reference
            .strip_prefix(SCHEMA_REF_PREFIX)
            .unwrap_or_else(|| panic!("unsupported $ref {reference} at {path}"));
        let target = root
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.get(name))
            .unwrap_or_else(|| panic!("$ref {reference} at {path} does not resolve"));
        return generate(
            root,
            target,
            kind,
            &format!("{SCHEMA_REF_PREFIX}{name}"),
            depth + 1,
        );
    }
    for construct in ["allOf", "oneOf", "anyOf"] {
        assert!(
            node.get(construct).is_none(),
            "unsupported construct {construct} at {path}"
        );
    }
    let declared = node.get("type").and_then(Value::as_str);
    let properties = node.get("properties").and_then(Value::as_object);
    match (declared, properties) {
        (Some("object"), _) | (None, Some(_)) => {
            generate_object(root, node, properties, kind, path, depth)
        }
        (Some("array"), _) => {
            let items = node
                .get("items")
                .unwrap_or_else(|| panic!("array without items at {path}"));
            json!([generate(
                root,
                items,
                kind,
                &format!("{path}/items"),
                depth + 1
            )])
        }
        (Some(scalar), _) => generate_scalar(node, scalar, path),
        (None, None) => panic!("unsupported schema node at {path}"),
    }
}

/// Builds an object document.
///
/// # Arguments
///
/// * `root` - The spec root.
/// * `node` - The object schema.
/// * `properties` - The `properties` map of `node`, when it has one.
/// * `kind` - Which properties the document holds.
/// * `path` - The place of `node` in the spec.
/// * `depth` - The nesting depth so far.
///
/// # Returns
///
/// An object with the properties that `kind` selects.
///
/// # Panics
///
/// Panics when a property schema is unsupported.
fn generate_object(
    root: &Value,
    node: &Value,
    properties: Option<&Map<String, Value>>,
    kind: Document,
    path: &str,
    depth: usize,
) -> Value {
    let required: Vec<&str> = node
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut object = Map::new();
    for (name, schema) in properties.into_iter().flatten() {
        if kind == Document::Full || required.contains(&name.as_str()) {
            let child = generate(root, schema, kind, &format!("{path}/{name}"), depth + 1);
            object.insert(name.clone(), child);
        }
    }
    Value::Object(object)
}

/// Builds a scalar document.
///
/// # Arguments
///
/// * `node` - The scalar schema.
/// * `scalar` - The declared `type` of `node`.
/// * `path` - The place of `node` in the spec.
///
/// # Returns
///
/// The `example` of the schema, else the first `enum` value, else a value
/// that fits the type and the format.
///
/// # Panics
///
/// Panics on a type the generator does not know.
fn generate_scalar(node: &Value, scalar: &str, path: &str) -> Value {
    if let Some(example) = node.get("example") {
        return example.clone();
    }
    match scalar {
        "string" => {
            if let Some(first) = node
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
            {
                return first.clone();
            }
            match node.get("format").and_then(Value::as_str) {
                Some("uuid") => json!("00000000-0000-0000-0000-000000000001"),
                Some("date-time") => json!("2026-01-01T00:00:00Z"),
                _ => json!("text"),
            }
        }
        "integer" => json!(1),
        "number" => json!(1.5),
        "boolean" => json!(true),
        other => panic!("unsupported type {other} at {path}"),
    }
}

/// Reads a document into a model.
///
/// # Arguments
///
/// * `document` - The document to read.
///
/// # Returns
///
/// `Ok` when the model `T` accepts the document, else the text of the error.
fn reads<T: DeserializeOwned>(document: &Value) -> Result<(), String> {
    serde_json::from_value::<T>(document.clone())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// One case: a schema name, a document kind, and the reader of its model.
type Case = (&'static str, Document, fn(&Value) -> Result<(), String>);

/// The cases that the guard runs.
const CASES: &[Case] = &[
    ("Site overview", Document::Minimal, reads::<models::Site>),
    ("Site overview", Document::Full, reads::<models::Site>),
    (
        "Site overview page",
        Document::Minimal,
        reads::<models::Page<models::Site>>,
    ),
    (
        "Site overview page",
        Document::Full,
        reads::<models::Page<models::Site>>,
    ),
    (
        "Device overview",
        Document::Minimal,
        reads::<models::Device>,
    ),
    ("Device overview", Document::Full, reads::<models::Device>),
    (
        "Device overview page",
        Document::Minimal,
        reads::<models::Page<models::Device>>,
    ),
    (
        "Device overview page",
        Document::Full,
        reads::<models::Page<models::Device>>,
    ),
    (
        "Latest statistics for a device",
        Document::Minimal,
        reads::<models::DeviceStatistics>,
    ),
    (
        "Voucher deletion results",
        Document::Full,
        reads::<models::VoucherDeletionResults>,
    ),
    (
        "Application info",
        Document::Minimal,
        reads::<models::ApplicationInfo>,
    ),
    (
        "Application info",
        Document::Full,
        reads::<models::ApplicationInfo>,
    ),
];

/// Every documented answer reads into its model, in both document kinds.
#[test]
fn each_documented_answer_reads_into_its_model() {
    let spec = load_spec();
    let mut failures = Vec::new();
    for (name, kind, read) in CASES {
        let schema = spec
            .get("components")
            .and_then(|components| components.get("schemas"))
            .and_then(|schemas| schemas.get(*name))
            .unwrap_or_else(|| panic!("the spec has no schema {name}"));
        let document = generate(&spec, schema, *kind, name, 0);
        if let Err(error) = read(&document) {
            failures.push(format!("{name} ({kind:?}): {error}"));
        }
    }
    assert!(
        failures.is_empty(),
        "models refuse documents that the spec allows:\n{}",
        failures.join("\n")
    );
}

/// A model that disagrees with its schema is refused in both kinds.
#[test]
fn a_model_that_disagrees_with_its_schema_is_refused() {
    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "the fields exist to be read by serde")]
    struct Wrong {
        note: String,
        count: String,
    }
    let schema = json!({
        "type": "object",
        "properties": {
            "note": {"type": "string"},
            "count": {"type": "integer"}
        },
        "required": ["count"]
    });
    let root = json!({});
    let minimal = generate(&root, &schema, Document::Minimal, "inline", 0);
    let full = generate(&root, &schema, Document::Full, "inline", 0);
    assert!(reads::<Wrong>(&minimal).is_err(), "minimal: {minimal}");
    assert!(reads::<Wrong>(&full).is_err(), "full: {full}");
}
