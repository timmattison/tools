//! The redaction filter: it finds text in the data of events and replaces it.
//!
//! The filter changes only the `data` of events. The number of events, their
//! order, their times, their types, and the length of the region stay the
//! same. An event whose data becomes empty stays in the list.
//!
//! # What the filter reads
//!
//! The filter reads output events always. It reads input events only when its
//! `input` option is true. The two kinds are two streams. A match never mixes
//! them, and an input event between two output events does not break an output
//! match.
//!
//! The terminal can split a word across events. Thus the filter joins the data
//! of the events of one stream, in order, and searches the joined text.
//!
//! # What the filter does not read
//!
//! The filter knows escape sequences, and it does not search them. It cuts the
//! joined data with these rules:
//!
//! - `ESC [` starts a CSI sequence. Parameter bytes (0x20 to 0x3F) follow, and
//!   one final byte (0x40 to 0x7E) ends it.
//! - `ESC ]` starts an OSC sequence, for example a window title. Its payload
//!   runs up to the first BEL or `ESC \`, or to the end of the stream. The
//!   introducer and the terminator are escape bytes. The payload is text, and
//!   it is a segment of its own.
//! - `ESC P`, `ESC X`, `ESC ^`, and `ESC _` start a DCS, SOS, PM, or APC
//!   string. It runs up to and including the next `ESC \`, or to the end of the
//!   stream. Nothing in it is text, because it can hold image data.
//! - Another `ESC` takes intermediate bytes (0x20 to 0x2F) and one final byte
//!   (0x30 to 0x7E), for example `ESC ( B` or `ESC 7`. If the final byte is
//!   missing, the sequence ends before the bad byte.
//! - Each other byte is text. Control characters such as carriage return,
//!   line feed, tab, and BEL are text.
//!
//! A sequence that the terminal split across two events is still one
//! sequence. The visible text is all text outside OSC payloads, joined across
//! escape sequences and across events. Thus `hun`, then `ESC [ 1 m`, then
//! `ter2` reads `hunter2`. Each OSC payload is a segment of its own. A match
//! never spans two segments. A search for `31` does not match inside
//! `ESC [ 31 m`.
//!
//! # What the filter changes
//!
//! A pattern with a capture group named `redact` replaces only the text of
//! that group. Else the filter replaces the whole match. It skips an empty
//! match. Escape bytes never change, also when a match crosses them.
//!
//! A replacement of the kind `Text` puts its text in the first piece of the
//! match. It makes each other piece empty. A piece is a part of a match that
//! lies inside one event and one run of text. The kinds `Asterisks` and
//! `Blocks` mask each piece. A control character stays as it is. Each run of
//! other characters becomes one mark for each terminal column, so a wide
//! character gives two marks and a combining mark gives none.
//!
//! A replacement text with a control character is not valid, because it
//! would change the state of the terminal.

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
    /// Returns `NleError::InvalidRedaction` when the text to find is empty,
    /// when a regular expression does not compile, or when a replacement text
    /// holds a control character.
    pub fn new(
        find: Find,
        replacement: Replacement,
        include_input: bool,
    ) -> Result<Self, NleError> {
        let invalid = |message: String| NleError::InvalidRedaction(message);
        let pattern = match &find {
            Find::Text(text) | Find::Regex(text) if text.is_empty() => {
                return Err(invalid("the text to find is empty".to_string()));
            }
            Find::Text(text) => regex::escape(text),
            Find::Regex(pattern) => pattern.clone(),
        };
        if let Replacement::Text(text) = &replacement {
            if text.chars().any(char::is_control) {
                return Err(invalid(
                    "the replacement holds a control character".to_string(),
                ));
            }
        }
        let matcher = regex::Regex::new(&pattern).map_err(|error| {
            invalid(format!("the regular expression does not compile: {error}"))
        })?;
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
