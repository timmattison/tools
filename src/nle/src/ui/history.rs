//! The undo and redo controls of the window: the Undo button and the Redo
//! button of the menu bar, and the keys Cmd+Z and Cmd+Shift+Z on macOS, or
//! Ctrl+Z and Ctrl+Shift+Z on the other platforms.
//!
//! Each control calls the undo or the redo of the `Editor` of the open
//! document. The preview, the timeline, and each panel read the project from
//! that editor, thus they show the project after the step at once.
//!
//! The root of the app gets the keys through [`window_shortcut`]. A key that a
//! text field gets stays in the text field, through [`keep_keys_in_field`],
//! thus Cmd+Z in a field undoes the typing and not an edit of the project.
//! [`key_forwarder`] sends a key that lands outside the app to its root.

use dioxus::html::ModifiersInteraction;
use dioxus::prelude::*;

use super::Document;

/// The text of the Undo button.
const UNDO_LABEL: &str = "Undo";
/// The text of the Redo button.
const REDO_LABEL: &str = "Redo";
/// The start of the message for a redo that the editor refuses.
const REDO_FAILED_PREFIX: &str = "Failed to redo";
/// The letter of the keys of Undo and Redo.
const SHORTCUT_LETTER: &str = "z";
/// The modifiers that make a key a shortcut. Shift is not one of them,
/// because it selects Redo in place of Undo.
const CHORD_MODIFIERS: Modifiers = Modifiers::ALT
    .union(Modifiers::CONTROL)
    .union(Modifiers::META);

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
    /// The modifier of the shortcuts on the platform that this build is for.
    pub(super) const HOST: Self = if cfg!(target_os = "macos") {
        Self::Command
    } else {
        Self::Control
    };

    /// The flag of this key in the modifiers of a key press.
    fn flag(self) -> Modifiers {
        match self {
            Self::Command => Modifiers::META,
            Self::Control => Modifiers::CONTROL,
        }
    }

    /// The text of the keys of `step`, for the tooltip of its button.
    pub(super) fn keys(self, step: HistoryStep) -> &'static str {
        match (self, step) {
            (Self::Command, HistoryStep::Undo) => "\u{2318}Z",
            (Self::Command, HistoryStep::Redo) => "\u{21e7}\u{2318}Z",
            (Self::Control, HistoryStep::Undo) => "Ctrl+Z",
            (Self::Control, HistoryStep::Redo) => "Ctrl+Shift+Z",
        }
    }
}

/// The step that a key press asks for, or `None` for a key that is no
/// shortcut on `platform`.
///
/// `key` is the value of the key and `code` is its place on the keyboard. The
/// letter Z counts, wherever the layout puts it. A layout with no Latin
/// letters, for example Russian, types a different letter on the Z key, and
/// then the place of the key counts. The modifier of `platform` must be down,
/// with no other modifier than Shift. Shift selects Redo.
pub(super) fn shortcut_of(
    key: &Key,
    code: Code,
    modifiers: Modifiers,
    platform: ShortcutModifier,
) -> Option<HistoryStep> {
    let Key::Character(text) = key else {
        return None;
    };
    let is_z =
        text.eq_ignore_ascii_case(SHORTCUT_LETTER) || (!text.is_ascii() && code == Code::KeyZ);
    if !is_z || modifiers.intersection(CHORD_MODIFIERS) != platform.flag() {
        return None;
    }
    Some(if modifiers.contains(Modifiers::SHIFT) {
        HistoryStep::Redo
    } else {
        HistoryStep::Undo
    })
}

/// The step that a key press on the window asks for, or `None`.
///
/// A press that asks for a step loses its default action, also when the
/// editor has no such step. Thus the Edit menu of macOS does not also send
/// the press to the undo of the page.
pub(super) fn window_shortcut(event: &KeyboardEvent) -> Option<HistoryStep> {
    let step = shortcut_of(
        &event.key(),
        event.code(),
        event.modifiers(),
        ShortcutModifier::HOST,
    )?;
    event.prevent_default();
    Some(step)
}

/// The key handler of each text field of the window.
///
/// A key that a text field gets is for the text, thus it does not go on to
/// the shortcuts of the window. The key keeps its default action, so Cmd+Z in
/// a field undoes the typing of the field, and the project does not change.
pub(super) fn keep_keys_in_field(event: KeyboardEvent) {
    event.stop_propagation();
}

/// The script for the head of the page that sends a key press with no target
/// in the app to the element with the id `root_id`, the root of the app.
///
/// When no element has the focus, the target of a key press is the body of
/// the page. Dioxus listens on its own root element, which is inside the
/// body, so such a press gets to no handler. This occurs on macOS after each
/// click on a button, because WebKit gives no focus to a button that gets a
/// click. It occurs on each platform when the element with the focus goes
/// away, for example the Remove button of a row.
///
/// The script sends a copy of such a press to the root. When a handler
/// prevents the default action of the copy, the script prevents the default
/// action of the press too. The copy has the root as its target, thus the
/// script does not send it again.
pub(super) fn key_forwarder(root_id: &str) -> String {
    format!(
        r#"<script>
document.addEventListener("keydown", (event) => {{
  const root = document.getElementById("{root_id}");
  if (root === null || root.contains(event.target)) {{
    return;
  }}
  const copy = new KeyboardEvent(event.type, event);
  root.dispatchEvent(copy);
  if (copy.defaultPrevented) {{
    event.preventDefault();
  }}
}});
</script>"#
    )
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
    let keys = ShortcutModifier::HOST;
    let undo_tooltip = format!("{UNDO_LABEL} ({})", keys.keys(HistoryStep::Undo));
    let redo_tooltip = format!("{REDO_LABEL} ({})", keys.keys(HistoryStep::Redo));
    rsx! {
        button {
            disabled: !can_undo,
            title: undo_tooltip,
            onclick: move |_| on_step.call(HistoryStep::Undo),
            {UNDO_LABEL}
        }
        button {
            disabled: !can_redo,
            title: redo_tooltip,
            onclick: move |_| on_step.call(HistoryStep::Redo),
            {REDO_LABEL}
        }
    }
}

#[cfg(test)]
mod tests;
