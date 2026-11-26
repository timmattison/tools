//! The chapter panel of the properties sidebar.

use dioxus::prelude::*;

use super::Document;

/// The chapter panel.
#[component]
pub(super) fn ChaptersPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let _ = (loaded, on_result);
    rsx! {
        div { class: "chapters-panel",
            h3 { "Chapters" }
        }
    }
}

#[cfg(test)]
mod tests;
