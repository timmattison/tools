//! A guard that reads the documented answers of the controller API into the models.
//!
//! The test builds a JSON document from each answer schema of
//! `integration.json` and deserializes it into the model that reads that
//! answer. A model that refuses a document the spec allows is a defect, except
//! for a pair of [`REFUSED_ON_PURPOSE`]. ufa cannot act on such a document, and
//! a test holds each of those pairs refused. A hand-written fixture repeats the
//! assumption of its author, so only a document built from the spec can
//! disagree with the model.
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
        (None, None) if is_empty_schema(node) => node.get("example").cloned().unwrap_or(json!({})),
        (None, None) => panic!("unsupported schema node at {path}"),
    }
}

/// The keys that give a schema node a shape. A node with none of them is the
/// empty schema, which allows any value.
const SHAPE_KEYWORDS: [&str; 8] = [
    "type",
    "properties",
    "items",
    "enum",
    "$ref",
    "allOf",
    "oneOf",
    "anyOf",
];

/// Whether `node` is the empty schema, which allows any value.
///
/// `Access point feature overview` is `{}` in the spec. A model reads it as
/// any value, so the generator gives the example of the node, or `{}`.
///
/// # Arguments
///
/// * `node` - The schema node.
///
/// # Returns
///
/// Whether `node` holds none of [`SHAPE_KEYWORDS`].
fn is_empty_schema(node: &Value) -> bool {
    SHAPE_KEYWORDS
        .iter()
        .all(|keyword| node.get(keyword).is_none())
}

/// Whether `example` has the JSON type that a scalar schema declares.
///
/// The spec gives some examples in a type other than the declared one: the
/// voucher `code` is a string with the example `4861409510`. A model follows
/// the declared type, so such an example is not a document the spec allows.
///
/// # Arguments
///
/// * `example` - The `example` of the schema node.
/// * `scalar` - The declared `type` of the node.
///
/// # Returns
///
/// Whether the example fits the declared type.
fn fits(example: &Value, scalar: &str) -> bool {
    match scalar {
        "string" => example.is_string(),
        "integer" => example.is_i64() || example.is_u64(),
        "number" => example.is_number(),
        "boolean" => example.is_boolean(),
        _ => false,
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
/// The `example` of the schema when it fits the declared type, else the
/// first `enum` value, else a value that fits the type and the format.
///
/// # Panics
///
/// Panics on a type the generator does not know.
fn generate_scalar(node: &Value, scalar: &str, path: &str) -> Value {
    if let Some(example) = node.get("example").filter(|example| fits(example, scalar)) {
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
        "Latest statistics for a device",
        Document::Full,
        reads::<models::DeviceStatistics>,
    ),
    (
        "Device details",
        Document::Minimal,
        reads::<models::DeviceDetails>,
    ),
    (
        "Device details",
        Document::Full,
        reads::<models::DeviceDetails>,
    ),
    (
        "Hotspot voucher details",
        Document::Minimal,
        reads::<models::Voucher>,
    ),
    (
        "Hotspot voucher details",
        Document::Full,
        reads::<models::Voucher>,
    ),
    (
        "Hotspot voucher detail page",
        Document::Minimal,
        reads::<models::Page<models::Voucher>>,
    ),
    (
        "Hotspot voucher detail page",
        Document::Full,
        reads::<models::Page<models::Voucher>>,
    ),
    (
        "IntegrationVoucherCreationResultDto",
        Document::Full,
        reads::<models::VoucherCreateResponse>,
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

/// The pairs that a model refuses on purpose, each with its reason.
///
/// The spec allows each of these documents, but ufa cannot act on it. The
/// model refuses it, and the command reports the answer as unreadable. A
/// test holds each pair refused, so a model that starts to read one fails
/// the guard until the pair moves into [`CASES`].
const REFUSED_ON_PURPOSE: &[(Case, &str)] = &[
    (
        (
            "IntegrationVoucherCreationResultDto",
            Document::Minimal,
            reads::<models::VoucherCreateResponse>,
        ),
        "ufa prints the vouchers that the answer lists, and an answer with no list has no vouchers to print",
    ),
    (
        (
            "Voucher deletion results",
            Document::Minimal,
            reads::<models::VoucherDeletionResults>,
        ),
        "ufa counts the deleted vouchers, and the deletion tally counts an answer with no count as unreadable",
    ),
];

/// Finds one schema of the spec by its name.
///
/// # Arguments
///
/// * `spec` - The spec root.
/// * `name` - The name of the schema under `components.schemas`.
///
/// # Returns
///
/// The schema node.
///
/// # Panics
///
/// Panics when the spec has no schema of that name.
fn schema<'a>(spec: &'a Value, name: &str) -> &'a Value {
    spec.get("components")
        .and_then(|components| components.get("schemas"))
        .and_then(|schemas| schemas.get(name))
        .unwrap_or_else(|| panic!("the spec has no schema {name}"))
}

/// Each pair of [`CASES`] reads into its model.
#[test]
fn each_documented_answer_reads_into_its_model() {
    let spec = load_spec();
    let mut failures = Vec::new();
    for (name, kind, read) in CASES {
        let document = generate(&spec, schema(&spec, name), *kind, name, 0);
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

/// Each pair of [`REFUSED_ON_PURPOSE`] is still refused by its model.
#[test]
fn each_refusal_on_purpose_is_still_refused() {
    let spec = load_spec();
    let mut reads_now = Vec::new();
    for ((name, kind, read), reason) in REFUSED_ON_PURPOSE {
        let document = generate(&spec, schema(&spec, name), *kind, name, 0);
        if read(&document).is_ok() {
            reads_now.push(format!(
                "{name} ({kind:?}) reads now, but it is refused on purpose because {reason}. Move it into CASES."
            ));
        }
    }
    assert!(
        reads_now.is_empty(),
        "a refusal on purpose is gone:\n{}",
        reads_now.join("\n")
    );
}

/// Each of the two disagreements is refused on its own: a model that requires
/// a field the schema leaves optional, and a model that types a field another
/// way. A model that agrees with the schema reads both kinds.
#[test]
fn a_model_that_disagrees_with_its_schema_is_refused() {
    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "the fields exist to be read by serde")]
    struct RequiresTheOptionalNote {
        note: String,
        count: u64,
    }
    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "the fields exist to be read by serde")]
    struct ReadsTheCountAsText {
        note: Option<String>,
        count: String,
    }
    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "the fields exist to be read by serde")]
    struct Agrees {
        note: Option<String>,
        count: u64,
    }
    let schema = json!({
        "type": "object",
        "properties": {
            "note": {"type": "string"},
            "count": {"type": "integer", "example": "a string example of an integer"}
        },
        "required": ["count"]
    });
    let root = json!({});
    let minimal = generate(&root, &schema, Document::Minimal, "inline", 0);
    let full = generate(&root, &schema, Document::Full, "inline", 0);

    assert!(
        reads::<RequiresTheOptionalNote>(&minimal).is_err(),
        "a missing optional field must be refused: {minimal}"
    );
    assert!(
        reads::<RequiresTheOptionalNote>(&full).is_ok(),
        "the full document holds the field: {full}"
    );
    assert!(
        reads::<ReadsTheCountAsText>(&full).is_err(),
        "a field of another type must be refused: {full}"
    );
    assert!(
        reads::<Agrees>(&minimal).is_ok() && reads::<Agrees>(&full).is_ok(),
        "a model that agrees reads both kinds, and an example of another type is not used: {minimal} {full}"
    );
}

/// The empty schema allows any value, and the generator gives its example.
#[test]
fn the_empty_schema_gives_its_example() {
    let root = json!({});
    let document = generate(&root, &json!({"example": {}}), Document::Full, "inline", 0);

    assert_eq!(document, json!({}));
}
