//! The undo and redo controls of the window: the Undo button and the Redo
//! button of the menu bar.
//!
//! Each control calls the undo or the redo of the `Editor` of the open
//! document. The preview, the timeline, and each panel read the project from
//! that editor, thus they show the project after the step at once.

use dioxus::prelude::*;

use super::Document;

/// The text of the Undo button.
const UNDO_LABEL: &str = "Undo";
/// The text of the Redo button.
const REDO_LABEL: &str = "Redo";
/// The start of the message for a redo that the editor refuses.
const REDO_FAILED_PREFIX: &str = "Failed to redo";

/// One step through the undo history of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HistoryStep {
    /// Go back to the project before the last edit.
    Undo,
    /// Apply again the last edit that an undo took back.
    Redo,
}

/// The modifier key that holds the shortcuts of a platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShortcutModifier {
    /// The Command key of macOS.
    Command,
    /// The Control key of each other platform.
    Control,
}

impl ShortcutModifier {
    /// The text of the keys of `step`, for the tooltip of its button.
    pub(super) fn keys(self, _step: HistoryStep) -> &'static str {
        ""
    }
}

/// The step that a key press asks for, or `None` for a key that is no
/// shortcut on `platform`.
pub(super) fn shortcut_of(
    _key: &Key,
    _code: Code,
    _modifiers: Modifiers,
    _platform: ShortcutModifier,
) -> Option<HistoryStep> {
    None
}

/// Makes `step` on the editor of the open document.
///
/// The answer is `Ok(true)` when the project changed, and `Ok(false)` when
/// there is no document or the editor has no such step. In that case the
/// signal gets no write, so the preview does not render the project again.
///
/// # Errors
/// Returns the text of the banner for a redo that the editor refuses. The
/// project then stays as it was.
pub(super) fn step_history(
    mut loaded: Signal<Option<Document>>,
    step: HistoryStep,
) -> Result<bool, String> {
    let possible = loaded.peek().as_ref().is_some_and(|open| match step {
        HistoryStep::Undo => open.editor.can_undo(),
        HistoryStep::Redo => open.editor.can_redo(),
    });
    if !possible {
        return Ok(false);
    }
    loaded.with_mut(|document| {
        document.as_mut().map_or(Ok(false), |open| match step {
            HistoryStep::Undo => Ok(open.editor.undo()),
            HistoryStep::Redo => open
                .editor
                .redo()
                .map_err(|failure| format!("{REDO_FAILED_PREFIX}: {failure}")),
        })
    })
}

/// The Undo button and the Redo button of the menu bar. Each one is disabled
/// when the editor of the open document has no such step, and when no
/// document is open. A click sends its step to `on_step`.
#[component]
pub(super) fn HistoryButtons(
    loaded: ReadSignal<Option<Document>>,
    on_step: EventHandler<HistoryStep>,
) -> Element {
    let (can_undo, can_redo) = loaded.read().as_ref().map_or((false, false), |open| {
        (open.editor.can_undo(), open.editor.can_redo())
    });
    rsx! {
        button {
            disabled: !can_undo,
            onclick: move |_| on_step.call(HistoryStep::Undo),
            {UNDO_LABEL}
        }
        button {
            disabled: !can_redo,
            onclick: move |_| on_step.call(HistoryStep::Redo),
            {REDO_LABEL}
        }
    }
}

#[cfg(test)]
mod tests;
