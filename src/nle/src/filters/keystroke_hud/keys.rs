//! The parser that turns the data of one input event into keys.

/// One key in the data of an input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// Printable text, as the user typed it.
    Text(String),
    /// A key that prints nothing, or a key with a modifier, by its label, for
    /// example `Enter`, `Ctrl+C`, `↑`, or `Alt+.`.
    Special(String),
}

/// The keys in the data of one input event, in order. Each run of printable
/// characters is one `Key::Text`. A byte sequence that is not a key (a mouse
/// report, an answer of the terminal) gives no key.
pub(crate) fn parse(data: &str) -> Vec<Key> {
    let _ = data;
    Vec::new()
}

#[cfg(test)]
mod tests;
