//! The chapter panel of the properties sidebar: the list of the chapter
//! markers of the project, a search field, and a form to add, edit, and remove
//! markers.
//!
//! The time of a marker comes from a text field, and it is on the timeline
//! before the filters. The panel does not read the playhead, because the
//! playhead is a time of the edited recording.

use dioxus::prelude::*;

use super::Document;
use crate::editor::Edit;
use crate::format::{format_clock, parse_clock};
use crate::types::{ChapterId, ChapterMarker, EditProject, Seconds};

/// The heading of the panel.
const CHAPTERS_HEADING: &str = "Chapters";
/// The label of the time field.
const TIME_LABEL: &str = "Time";
/// The example in the time field.
const TIME_PLACEHOLDER: &str = "1:23.5";
/// The label of the label field.
const LABEL_LABEL: &str = "Label";
/// The text of the button that adds a chapter.
const ADD_LABEL: &str = "Add Chapter";
/// The text of the button that saves the chapter that is edited.
const SAVE_LABEL: &str = "Save Chapter";
/// The text of the button that ends an edit.
const CANCEL_LABEL: &str = "Cancel";
/// The example in the search field.
const SEARCH_PLACEHOLDER: &str = "Search chapters";
/// The accessible name of the search field.
const SEARCH_ARIA_LABEL: &str = "Search chapters";
/// The text of a project with no marker.
const NO_CHAPTERS: &str = "No chapters yet";
/// The text of a search that matches no marker.
const NO_MATCH: &str = "No chapter matches";
/// The note on a marker that is after the end of the project.
const AFTER_THE_END: &str = "after the end";
/// The text of the button that edits a marker.
const EDIT_LABEL: &str = "Edit";
/// The text of the button that removes a marker.
const REMOVE_LABEL: &str = "Remove";
/// The start of the message for a chapter change that failed.
const FAILED_PREFIX: &str = "Failed to change the chapters";
/// The reason for a label with no visible character.
const LABEL_EMPTY: &str = "the label is empty";

/// One row of the list: a marker that matches the search.
struct Row {
    id: ChapterId,
    /// The time of the marker, exact.
    time: Seconds,
    label: String,
    /// Whether the time is after the end of the project.
    late: bool,
}

/// What the list shows: the count of markers of the project, and the rows of
/// the markers that match the search, in time order.
struct Listing {
    total: usize,
    rows: Vec<Row>,
}

/// Whether `label` matches the search text. A blank search matches all.
fn matches_search(label: &str, search: &str) -> bool {
    let search = search.trim();
    search.is_empty() || label.to_lowercase().contains(&search.to_lowercase())
}

/// Builds the list of the open document for the search text. With no open
/// document, the list is empty.
fn listing(document: Option<&Document>, search: &str) -> Listing {
    let Some(open) = document else {
        return Listing {
            total: 0,
            rows: Vec::new(),
        };
    };
    let project = open.editor.project();
    let end = project.duration();
    let rows = project
        .chapters_in_order()
        .into_iter()
        .filter(|marker| matches_search(&marker.label, search))
        .map(|marker| Row {
            id: marker.id,
            time: marker.time,
            label: marker.label.clone(),
            late: marker.time.get() > end,
        })
        .collect();
    Listing {
        total: project.chapters.len(),
        rows,
    }
}

/// Reads the time and the label that the user typed. The label loses the
/// white space at its ends.
fn read_fields(time: &str, label: &str) -> Result<(Seconds, String), String> {
    let time = parse_clock(time)?;
    let label = label.trim();
    if label.is_empty() {
        return Err(LABEL_EMPTY.to_owned());
    }
    Ok((time, label.to_owned()))
}

/// Applies the edit that `make` builds from the project to the editor of the
/// open document. The answer is the text of the error of a refused edit. With
/// no open document, nothing changes and the answer is `Ok`.
fn edit_document(
    mut loaded: Signal<Option<Document>>,
    make: impl FnOnce(&EditProject) -> Edit,
) -> Result<(), String> {
    // One write borrow holds the whole edit. No read borrow lives across it.
    loaded.with_mut(|document| {
        document.as_mut().map_or(Ok(()), |open| {
            let edit = make(open.editor.project());
            open.editor
                .apply(edit)
                .map_err(|failure| failure.to_string())
        })
    })
}

/// Sends the outcome of a chapter change to `on_result`: `Ok` for a change
/// that applied, or the message of the banner for a change that did not.
fn report(on_result: EventHandler<Result<(), String>>, outcome: Result<(), String>) {
    on_result.call(outcome.map_err(|reason| format!("{FAILED_PREFIX}: {reason}")));
}

/// The chapter panel: the form, the search field, and one row for each marker
/// that matches the search. `on_result` gets the outcome of each Add Chapter,
/// Save Chapter, and Remove. Edit and Cancel change no project, so they send
/// nothing.
#[component]
pub(super) fn ChaptersPanel(
    loaded: Signal<Option<Document>>,
    on_result: EventHandler<Result<(), String>>,
) -> Element {
    let mut time_text = use_signal(String::new);
    let mut label_text = use_signal(String::new);
    let mut search = use_signal(String::new);
    let mut editing = use_signal(|| None::<ChapterId>);
    let Listing { total, rows } = listing(loaded.read().as_ref(), &search.read());
    let button_label = if editing().is_some() {
        SAVE_LABEL
    } else {
        ADD_LABEL
    };
    rsx! {
        div { class: "chapters-panel",
            h3 { {CHAPTERS_HEADING} }
            label { class: "chapter-field",
                {TIME_LABEL}
                input {
                    r#type: "text",
                    placeholder: TIME_PLACEHOLDER,
                    value: "{time_text}",
                    oninput: move |event| time_text.set(event.value()),
                }
            }
            label { class: "chapter-field",
                {LABEL_LABEL}
                input {
                    r#type: "text",
                    value: "{label_text}",
                    oninput: move |event| label_text.set(event.value()),
                }
            }
            button {
                onclick: move |_| {
                    let outcome = read_fields(&time_text.read(), &label_text.read())
                        .and_then(|(time, label)| {
                            let target = editing();
                            edit_document(loaded, |project| match target {
                                Some(id) => Edit::UpdateChapter { id, time, label },
                                None => Edit::AddChapter {
                                    chapter: ChapterMarker {
                                        id: project.next_chapter_id(),
                                        time,
                                        label,
                                    },
                                },
                            })
                        });
                    if outcome.is_ok() {
                        time_text.set(String::new());
                        label_text.set(String::new());
                        editing.set(None);
                    }
                    report(on_result, outcome);
                },
                {button_label}
            }
            if editing().is_some() {
                button {
                    onclick: move |_| {
                        time_text.set(String::new());
                        label_text.set(String::new());
                        editing.set(None);
                    },
                    {CANCEL_LABEL}
                }
            }
            input {
                r#type: "search",
                placeholder: SEARCH_PLACEHOLDER,
                aria_label: SEARCH_ARIA_LABEL,
                oninput: move |event| search.set(event.value()),
            }
            if total == 0 {
                p { class: "placeholder", {NO_CHAPTERS} }
            } else if rows.is_empty() {
                p { class: "placeholder", {NO_MATCH} }
            }
            for row in rows {
                div { class: "chapter-row", key: "{row.id.0}",
                    span { class: "chapter-time", {format_clock(row.time.get(), true)} }
                    span { class: "chapter-label", "{row.label}" }
                    if row.late {
                        span { class: "chapter-late", {AFTER_THE_END} }
                    }
                    button {
                        onclick: {
                            let label = row.label.clone();
                            move |_| {
                                editing.set(Some(row.id));
                                time_text.set(format!("{}", row.time.get()));
                                label_text.set(label.clone());
                            }
                        },
                        {EDIT_LABEL}
                    }
                    button {
                        onclick: move |_| {
                            let id = row.id;
                            let outcome = edit_document(loaded, |_| Edit::RemoveChapter { id });
                            if outcome.is_ok() && editing() == Some(id) {
                                time_text.set(String::new());
                                label_text.set(String::new());
                                editing.set(None);
                            }
                            report(on_result, outcome);
                        },
                        {REMOVE_LABEL}
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
