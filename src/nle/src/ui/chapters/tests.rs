//! Tests of the chapter panel. Each test renders the panel to text with the
//! Dioxus SSR renderer, and drives its fields and buttons with events.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

use dioxus::prelude::*;
use dioxus_core::{ElementId, Mutation};

use super::{ChaptersPanel, Document};
use crate::pipeline::FilterRegistry;
use crate::types::{ChapterId, ChapterMarker, Seconds};
use crate::ui::tests::fixture;

/// The start of the message of a chapter action that failed.
const FAILED: &str = "Failed to change the chapters";

type Outcomes = Rc<RefCell<Vec<Result<(), String>>>>;
type Markers = Rc<RefCell<Vec<ChapterMarker>>>;

/// What the test root builds its document from, and where it reports to.
#[derive(Clone)]
struct Setup {
    /// The time and the label of each marker that the document starts with.
    markers: Vec<(f64, &'static str)>,
    /// The results that the panel sent to `on_result`.
    outcomes: Outcomes,
    /// The markers of the project in the editor, as of the newest render.
    chapters: Markers,
}

/// A root that holds a document of 65.3 seconds and shows the chapter panel.
fn root() -> Element {
    let setup = use_context::<Setup>();
    let loaded = use_signal(|| {
        let mut document =
            Document::open(fixture(), Arc::new(FilterRegistry::new())).expect("a document");
        for (index, (time, label)) in setup.markers.iter().enumerate() {
            let chapter = ChapterMarker {
                id: ChapterId(index as u64 + 1),
                time: Seconds::new(*time).expect("a time"),
                label: (*label).to_owned(),
            };
            document
                .editor
                .apply(crate::editor::Edit::AddChapter { chapter })
                .expect("a chapter that applies");
        }
        Some(document)
    });
    *setup.chapters.borrow_mut() = loaded
        .read()
        .as_ref()
        .map(|open| open.editor.project().chapters.clone())
        .unwrap_or_default();
    let outcomes = setup.outcomes;
    rsx! {
        ChaptersPanel {
            loaded,
            on_result: move |result| outcomes.borrow_mut().push(result),
        }
    }
}

/// A rendered panel that a test drives.
struct Panel {
    dom: VirtualDom,
    /// The id of each click listener, in creation order.
    clicks: Vec<ElementId>,
    /// The id of each input listener, in creation order.
    inputs: Vec<ElementId>,
    /// The click listeners that the newest render created.
    newest_clicks: Vec<ElementId>,
    outcomes: Outcomes,
    chapters: Markers,
}

impl Panel {
    /// Renders the panel of a project that holds `markers`.
    fn open(markers: &[(f64, &'static str)]) -> Self {
        static CONVERTER: Once = Once::new();
        CONVERTER.call_once(|| {
            dioxus::html::set_event_converter(Box::new(dioxus::html::SerializedHtmlEventConverter));
        });
        let outcomes = Outcomes::default();
        let chapters = Markers::default();
        let setup = Setup {
            markers: markers.to_vec(),
            outcomes: Rc::clone(&outcomes),
            chapters: Rc::clone(&chapters),
        };
        let mut dom = VirtualDom::new(root).with_root_context(setup);
        let edits = dom.rebuild_to_vec().edits;
        let mut panel = Self {
            dom,
            clicks: Vec::new(),
            inputs: Vec::new(),
            newest_clicks: Vec::new(),
            outcomes,
            chapters,
        };
        panel.note_listeners(&edits);
        panel
    }

    /// Keeps the id of each new listener in `edits`.
    fn note_listeners(&mut self, edits: &[Mutation]) {
        let mut created_clicks = Vec::new();
        for edit in edits {
            if let Mutation::NewEventListener { name, id } = edit {
                match name.as_str() {
                    "click" => created_clicks.push(*id),
                    "input" => self.inputs.push(*id),
                    _ => {}
                }
            }
        }
        self.clicks.extend(&created_clicks);
        self.newest_clicks = created_clicks;
    }

    /// Renders what is dirty.
    fn render(&mut self) {
        let edits = self.dom.render_immediate_to_vec().edits;
        self.note_listeners(&edits);
    }

    /// Sends a click to the element and renders.
    fn click(&mut self, id: ElementId) {
        let data = Rc::new(PlatformEventData::new(Box::<SerializedMouseData>::default()));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("click", event, id);
        self.render();
    }

    /// Sends the text to the input element, as if the user typed it, and renders.
    fn type_into(&mut self, id: ElementId, text: &str) {
        let form = dioxus::html::SerializedFormData::new(text.to_owned(), Vec::new());
        let data = Rc::new(PlatformEventData::new(Box::new(form)));
        let event = dioxus::prelude::Event::new(data as Rc<dyn Any>, true);
        self.dom.runtime().handle_event("input", event, id);
        self.render();
    }

    /// The time field, the label field, and the search field are the three
    /// inputs, in this order.
    fn field(&self, index: usize) -> ElementId {
        assert_eq!(self.inputs.len(), 3, "{:?}", self.inputs);
        self.inputs[index]
    }

    fn time_field(&self) -> ElementId {
        self.field(0)
    }

    fn label_field(&self) -> ElementId {
        self.field(1)
    }

    fn search_field(&self) -> ElementId {
        self.field(2)
    }

    /// The Add Chapter button, which is also the Save Chapter button. It is
    /// the first click listener of the first render.
    fn add_button(&self) -> ElementId {
        self.clicks[0]
    }

    /// The Edit button of the row at `row` in the first render. The buttons of
    /// a row are Edit and Remove, after the Add Chapter button.
    fn edit_button(&self, row: usize) -> ElementId {
        self.clicks[1 + 2 * row]
    }

    /// The Remove button of the row at `row` in the first render.
    fn remove_button(&self, row: usize) -> ElementId {
        self.clicks[2 + 2 * row]
    }

    /// The Cancel button. It is the one listener that the render of the Edit
    /// click created.
    fn cancel_button(&self) -> ElementId {
        assert_eq!(self.newest_clicks.len(), 1, "{:?}", self.newest_clicks);
        self.newest_clicks[0]
    }

    /// Types a time and a label, and clicks the Add Chapter button.
    fn add(&mut self, time: &str, label: &str) {
        self.type_into(self.time_field(), time);
        self.type_into(self.label_field(), label);
        self.click(self.add_button());
    }

    fn html(&self) -> String {
        dioxus_ssr::render(&self.dom)
    }

    /// The markers of the project in the editor.
    fn markers(&self) -> Vec<(ChapterId, f64, String)> {
        self.chapters
            .borrow()
            .iter()
            .map(|marker| (marker.id, marker.time.get(), marker.label.clone()))
            .collect()
    }

    fn outcomes(&self) -> Vec<Result<(), String>> {
        self.outcomes.borrow().clone()
    }
}

/// The text of the row of a marker, as the page shows it.
fn row_text(time: &str, label: &str) -> String {
    format!(
        "<span class=\"chapter-time\">{time}</span><span class=\"chapter-label\">{label}</span>"
    )
}

/// The number of rows on the page.
fn row_count(page: &str) -> usize {
    page.matches("class=\"chapter-row\"").count()
}

/// The message that a refused chapter action sends.
fn failure(reason: &str) -> Result<(), String> {
    Err(format!("{FAILED}: {reason}"))
}

#[test]
fn a_project_with_no_marker_shows_the_empty_text_and_no_row() {
    let panel = Panel::open(&[]);
    let page = panel.html();
    assert!(page.contains("Chapters"), "{page}");
    assert!(page.contains("No chapters yet"), "{page}");
    assert_eq!(row_count(&page), 0, "{page}");
    assert!(page.contains(">Add Chapter</button>"), "{page}");
    assert!(!page.contains(">Cancel</button>"), "{page}");
    assert!(!page.contains("No chapter matches"), "{page}");
}

#[test]
fn the_rows_come_in_time_order_with_clock_times_and_labels() {
    let panel = Panel::open(&[(50.0, "Build"), (5.0, "日本語テスト"), (65.0, "café 🎉")]);
    let page = panel.html();
    assert_eq!(row_count(&page), 3, "{page}");
    assert!(!page.contains("No chapters yet"), "{page}");
    let rows = [
        row_text("0:05.0", "日本語テスト"),
        row_text("0:50.0", "Build"),
        row_text("1:05.0", "café 🎉"),
    ];
    let mut rest = page.as_str();
    for row in rows {
        rest = rest
            .split_once(&row)
            .unwrap_or_else(|| panic!("{row} is missing or out of order in {page}"))
            .1;
    }
}

#[test]
fn a_marker_after_the_end_of_the_project_says_so() {
    let panel = Panel::open(&[(70.0, "Late"), (65.0, "On time")]);
    let page = panel.html();
    assert_eq!(page.matches("class=\"chapter-late\"").count(), 1, "{page}");
    assert_eq!(page.matches("after the end").count(), 1, "{page}");
    let (before_late, from_late) = page.split_once("1:10.0").expect("the late row");
    assert!(before_late.contains("1:05.0"), "{page}");
    assert!(!before_late.contains("after the end"), "{page}");
    assert!(from_late.contains("after the end"), "{page}");
}

#[test]
fn the_search_shows_only_the_matching_rows() {
    let mut panel = Panel::open(&[(1.0, "Build"), (2.0, "Test"), (3.0, "Build again")]);
    panel.type_into(panel.search_field(), "build");
    let page = panel.html();
    assert_eq!(row_count(&page), 2, "{page}");
    assert!(page.contains(">Build<"), "{page}");
    assert!(page.contains(">Build again<"), "{page}");
    assert!(!page.contains(">Test<"), "{page}");
}

#[test]
fn the_search_ignores_case_and_trims_the_text() {
    let mut panel = Panel::open(&[(1.0, "Build"), (2.0, "Test")]);
    panel.type_into(panel.search_field(), "  BUILD ");
    let page = panel.html();
    assert_eq!(row_count(&page), 1, "{page}");
    assert!(page.contains(">Build<"), "{page}");
}

#[test]
fn the_search_ignores_case_of_a_multi_byte_label() {
    let mut panel = Panel::open(&[(1.0, "café 🎉"), (2.0, "日本語テスト")]);
    panel.type_into(panel.search_field(), "CAFÉ");
    let page = panel.html();
    assert_eq!(row_count(&page), 1, "{page}");
    assert!(page.contains(">café 🎉<"), "{page}");
    panel.type_into(panel.search_field(), "本語");
    let page = panel.html();
    assert_eq!(row_count(&page), 1, "{page}");
    assert!(page.contains(">日本語テスト<"), "{page}");
}

#[test]
fn a_search_with_no_match_says_so_and_a_blank_search_shows_all() {
    let mut panel = Panel::open(&[(1.0, "Build")]);
    panel.type_into(panel.search_field(), "zzz");
    let page = panel.html();
    assert!(page.contains("No chapter matches"), "{page}");
    assert!(!page.contains("No chapters yet"), "{page}");
    assert_eq!(row_count(&page), 0, "{page}");
    panel.type_into(panel.search_field(), "   ");
    let page = panel.html();
    assert!(!page.contains("No chapter matches"), "{page}");
    assert_eq!(row_count(&page), 1, "{page}");
}

#[test]
fn adding_a_chapter_changes_the_project_shows_the_row_and_clears_the_fields() {
    let mut panel = Panel::open(&[]);
    panel.add("1:23.5", "  Intro  ");
    assert_eq!(panel.markers(), vec![(ChapterId(1), 83.5, "Intro".into())]);
    let page = panel.html();
    assert_eq!(row_count(&page), 1, "{page}");
    assert!(page.contains(&row_text("1:23.5", "Intro")), "{page}");
    assert!(!page.contains("No chapters yet"), "{page}");
    assert!(!page.contains("value=\"1:23.5\""), "{page}");
    assert!(!page.contains("value=\"  Intro  \""), "{page}");
    assert_eq!(page.matches("value=\"\"").count(), 2, "{page}");
    assert_eq!(panel.outcomes(), vec![Ok(())]);
}

#[test]
fn an_added_chapter_gets_the_next_id() {
    let mut panel = Panel::open(&[(1.0, "One"), (2.0, "Two")]);
    panel.add("3", "Three");
    let ids: Vec<ChapterId> = panel.markers().iter().map(|marker| marker.0).collect();
    assert_eq!(ids, vec![ChapterId(1), ChapterId(2), ChapterId(3)]);
}

#[test]
fn a_bad_time_adds_nothing_and_keeps_the_text() {
    let mut panel = Panel::open(&[]);
    panel.add("abc", "Intro");
    assert!(panel.markers().is_empty());
    let page = panel.html();
    assert!(page.contains("value=\"abc\""), "{page}");
    assert!(page.contains("value=\"Intro\""), "{page}");
    assert_eq!(
        panel.outcomes(),
        vec![failure(
            "the time \"abc\" is not in seconds, M:SS, or H:MM:SS"
        )]
    );
}

#[test]
fn an_empty_time_adds_nothing() {
    let mut panel = Panel::open(&[]);
    panel.type_into(panel.label_field(), "Intro");
    panel.click(panel.add_button());
    assert!(panel.markers().is_empty());
    assert_eq!(panel.outcomes(), vec![failure("the time is empty")]);
}

#[test]
fn a_blank_label_adds_nothing_and_keeps_the_text() {
    let mut panel = Panel::open(&[]);
    panel.add("5", "   ");
    assert!(panel.markers().is_empty());
    let page = panel.html();
    assert!(page.contains("value=\"5\""), "{page}");
    assert_eq!(panel.outcomes(), vec![failure("the label is empty")]);
}

#[test]
fn a_failure_then_a_good_action_sends_the_error_then_ok() {
    let mut panel = Panel::open(&[]);
    panel.click(panel.add_button());
    panel.add("5", "Intro");
    let outcomes = panel.outcomes();
    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    assert!(outcomes[0].is_err(), "{outcomes:?}");
    assert_eq!(outcomes[1], Ok(()));
}

#[test]
fn edit_fills_the_fields_with_the_exact_seconds_and_the_label() {
    let mut panel = Panel::open(&[(12.345, "café 🎉")]);
    panel.click(panel.edit_button(0));
    let page = panel.html();
    assert!(page.contains("value=\"12.345\""), "{page}");
    assert!(page.contains("value=\"café 🎉\""), "{page}");
    assert!(page.contains(">Save Chapter</button>"), "{page}");
    assert!(!page.contains(">Add Chapter</button>"), "{page}");
    assert!(page.contains(">Cancel</button>"), "{page}");
    assert!(panel.outcomes().is_empty());
}

#[test]
fn save_chapter_changes_the_marker_and_goes_back_to_add() {
    let mut panel = Panel::open(&[(12.345, "Build"), (20.0, "Test")]);
    panel.click(panel.edit_button(0));
    panel.type_into(panel.label_field(), " Built ");
    panel.type_into(panel.time_field(), "0:30");
    panel.click(panel.add_button());
    assert_eq!(
        panel.markers(),
        vec![
            (ChapterId(1), 30.0, "Built".into()),
            (ChapterId(2), 20.0, "Test".into())
        ]
    );
    let page = panel.html();
    assert!(page.contains(">Add Chapter</button>"), "{page}");
    assert!(!page.contains(">Save Chapter</button>"), "{page}");
    assert!(!page.contains(">Cancel</button>"), "{page}");
    assert_eq!(page.matches("value=\"\"").count(), 2, "{page}");
    assert_eq!(panel.outcomes(), vec![Ok(())]);
}

#[test]
fn save_with_no_change_keeps_the_exact_time() {
    let mut panel = Panel::open(&[(12.345, "Build")]);
    panel.click(panel.edit_button(0));
    panel.click(panel.add_button());
    assert_eq!(
        panel.markers(),
        vec![(ChapterId(1), 12.345, "Build".into())]
    );
}

#[test]
fn a_bad_save_keeps_the_marker_and_the_edit() {
    let mut panel = Panel::open(&[(12.0, "Build")]);
    panel.click(panel.edit_button(0));
    panel.type_into(panel.time_field(), "1:99");
    panel.click(panel.add_button());
    assert_eq!(panel.markers(), vec![(ChapterId(1), 12.0, "Build".into())]);
    let page = panel.html();
    assert!(page.contains(">Save Chapter</button>"), "{page}");
    assert!(page.contains("value=\"1:99\""), "{page}");
    assert_eq!(
        panel.outcomes(),
        vec![failure(
            "the time \"1:99\" is not in seconds, M:SS, or H:MM:SS"
        )]
    );
}

#[test]
fn cancel_clears_the_fields_and_goes_back_to_add_with_no_change() {
    let mut panel = Panel::open(&[(12.0, "Build")]);
    panel.click(panel.edit_button(0));
    let cancel = panel.cancel_button();
    panel.type_into(panel.label_field(), "Changed");
    panel.click(cancel);
    assert_eq!(panel.markers(), vec![(ChapterId(1), 12.0, "Build".into())]);
    let page = panel.html();
    assert!(page.contains(">Add Chapter</button>"), "{page}");
    assert!(!page.contains(">Cancel</button>"), "{page}");
    assert!(!page.contains("value=\"Changed\""), "{page}");
    assert_eq!(page.matches("value=\"\"").count(), 2, "{page}");
}

#[test]
fn remove_removes_the_marker_and_its_row() {
    let mut panel = Panel::open(&[(1.0, "One"), (2.0, "Two"), (3.0, "Three")]);
    panel.click(panel.remove_button(1));
    assert_eq!(
        panel.markers(),
        vec![
            (ChapterId(1), 1.0, "One".into()),
            (ChapterId(3), 3.0, "Three".into())
        ]
    );
    let page = panel.html();
    assert_eq!(row_count(&page), 2, "{page}");
    assert!(!page.contains(">Two<"), "{page}");
    assert_eq!(panel.outcomes(), vec![Ok(())]);
}

#[test]
fn removing_the_last_marker_shows_the_empty_text() {
    let mut panel = Panel::open(&[(1.0, "One")]);
    panel.click(panel.remove_button(0));
    assert!(panel.markers().is_empty());
    assert!(panel.html().contains("No chapters yet"));
}

#[test]
fn removing_the_marker_that_is_edited_ends_the_edit() {
    let mut panel = Panel::open(&[(12.0, "Build")]);
    panel.click(panel.edit_button(0));
    panel.click(panel.remove_button(0));
    let page = panel.html();
    assert!(page.contains(">Add Chapter</button>"), "{page}");
    assert!(!page.contains(">Cancel</button>"), "{page}");
    assert!(!page.contains("value=\"Build\""), "{page}");
    assert_eq!(page.matches("value=\"\"").count(), 2, "{page}");
}

#[test]
fn removing_another_marker_keeps_the_edit() {
    let mut panel = Panel::open(&[(1.0, "One"), (2.0, "Two")]);
    panel.click(panel.edit_button(0));
    panel.click(panel.remove_button(1));
    let page = panel.html();
    assert!(page.contains(">Save Chapter</button>"), "{page}");
    assert!(page.contains("value=\"One\""), "{page}");
}
