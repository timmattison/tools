//! The redaction filter: it finds text in the data of events and replaces it.

use serde::{Deserialize, Serialize};

mod stream;

use crate::error::NleError;
use crate::pipeline::{FilterRegistry, Span};
use crate::types::{AppliedFilter, FilterKind, FilterOptions, FilterRegion};

/// The kind of the redaction filter.
pub const KIND: &str = "redaction";

/// The option that holds what to find.
pub const FIND_OPTION: &str = "find";

/// The option that holds the replacement.
pub const REPLACE_OPTION: &str = "replace";

/// The option that says if input events are redacted too.
pub const INPUT_OPTION: &str = "input";

/// The name of the capture group whose text alone is replaced.
pub const REDACT_GROUP: &str = "redact";

/// The character of the `Asterisks` replacement.
pub const ASTERISK: char = '*';

/// The character of the `Blocks` replacement. It is U+2588 FULL BLOCK.
pub const BLOCK: char = '\u{2588}';

/// What to find.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Find {
    /// This exact text.
    Text(String),
    /// A regular expression in the syntax of the `regex` crate.
    Regex(String),
}

/// What to put in place of a match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum Replacement {
    /// This text, in place of the match.
    Text(String),
    /// One `*` for each terminal column of the match.
    Asterisks,
    /// One `█` for each terminal column of the match.
    Blocks,
}

/// One find and replace that is known to be valid.
#[derive(Debug, Clone)]
pub struct Redaction {
    find: Find,
    replacement: Replacement,
    include_input: bool,
    matcher: regex::Regex,
}

impl PartialEq for Redaction {
    fn eq(&self, other: &Self) -> bool {
        self.find == other.find
            && self.replacement == other.replacement
            && self.include_input == other.include_input
    }
}

impl Redaction {
    /// Make a redaction.
    ///
    /// # Errors
    /// Returns `NleError::InvalidRedaction` when the pattern does not compile.
    pub fn new(
        find: Find,
        replacement: Replacement,
        include_input: bool,
    ) -> Result<Self, NleError> {
        let pattern = match &find {
            Find::Text(text) => regex::escape(text),
            Find::Regex(pattern) => pattern.clone(),
        };
        let matcher = regex::Regex::new(&pattern)
            .map_err(|error| NleError::InvalidRedaction(error.to_string()))?;
        Ok(Self {
            find,
            replacement,
            include_input,
            matcher,
        })
    }

    /// What this redaction finds.
    #[must_use]
    pub fn find(&self) -> &Find {
        &self.find
    }

    /// What this redaction puts in place of a match.
    #[must_use]
    pub fn replacement(&self) -> &Replacement {
        &self.replacement
    }

    /// Whether this redaction also changes input events.
    #[must_use]
    pub fn includes_input(&self) -> bool {
        self.include_input
    }
}

/// The kind of the redaction filter, as a `FilterKind`.
#[must_use]
pub fn kind() -> FilterKind {
    FilterKind::new(KIND)
}

/// A redaction filter for `region`.
#[must_use]
pub fn applied(region: FilterRegion, redaction: &Redaction) -> AppliedFilter {
    let value = |value: Result<serde_json::Value, serde_json::Error>| {
        value.unwrap_or(serde_json::Value::Null)
    };
    AppliedFilter {
        kind: kind(),
        region,
        options: FilterOptions::new()
            .with(FIND_OPTION, value(serde_json::to_value(&redaction.find)))
            .with(
                REPLACE_OPTION,
                value(serde_json::to_value(&redaction.replacement)),
            )
            .with(INPUT_OPTION, redaction.include_input),
    }
}

/// The redaction of a redaction filter. It is `None` when the filter is of
/// another kind, or when its options are missing or not valid.
#[must_use]
pub fn redaction_of(filter: &AppliedFilter) -> Option<Redaction> {
    if filter.kind != kind() {
        return None;
    }
    read_options(&filter.options).ok()
}

/// Add the redaction filter to `registry`.
pub fn register(registry: &mut FilterRegistry) {
    registry.register_span(kind(), redact);
}

/// Read the three options of the filter.
fn read_options(options: &FilterOptions) -> Result<Redaction, String> {
    let find = required::<Find>(options, FIND_OPTION)?;
    let replacement = required::<Replacement>(options, REPLACE_OPTION)?;
    let include_input = match options.get(INPUT_OPTION) {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| missing(INPUT_OPTION))?,
    };
    Redaction::new(find, replacement, include_input).map_err(|error| error.to_string())
}

/// Read one required option.
fn required<T: serde::de::DeserializeOwned>(
    options: &FilterOptions,
    key: &str,
) -> Result<T, String> {
    options
        .get(key)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .ok_or_else(|| missing(key))
}

/// The error text for a missing or bad option.
fn missing(key: &str) -> String {
    format!("the option \"{key}\" is missing or is not valid")
}

/// Replace the text that the options name.
fn redact(span: Span, options: &FilterOptions) -> Result<Span, String> {
    let redaction = read_options(options)?;
    Ok(Span {
        events: stream::rewrite(span.events, &redaction)?,
        length: span.length,
    })
}

#[cfg(test)]
mod tests;
