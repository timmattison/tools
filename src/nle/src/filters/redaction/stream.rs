//! The search and the rewrite of the event data.

use crate::filters::redaction::{Redaction, Replacement};
use crate::types::{Event, EventType};

/// Replace each match in the output events of `events`.
pub(super) fn rewrite(events: Vec<Event>, redaction: &Redaction) -> Result<Vec<Event>, String> {
    let Replacement::Text(replacement) = redaction.replacement() else {
        return Ok(events);
    };
    Ok(events
        .into_iter()
        .map(|event| {
            if event.event_type == EventType::Output {
                let data = redaction
                    .matcher
                    .replace_all(&event.data, regex::NoExpand(replacement))
                    .into_owned();
                Event { data, ..event }
            } else {
                event
            }
        })
        .collect())
}
