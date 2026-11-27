//! Tests of the rule that turns a key press into a step of the undo history.
//! Each test states the platform, so the rule of each platform runs on each
//! host.

use super::*;

use HistoryStep::{Redo, Undo};
use ShortcutModifier::{Command, Control};

/// The key value of a character.
fn character(text: &str) -> Key {
    Key::Character(text.to_owned())
}

/// The step of a press of the key with the value `text` at the place `code`.
fn step(
    text: &str,
    code: Code,
    modifiers: Modifiers,
    platform: ShortcutModifier,
) -> Option<HistoryStep> {
    shortcut_of(&character(text), code, modifiers, platform)
}

#[test]
fn command_z_undoes_and_command_shift_z_redoes_on_macos() {
    assert_eq!(step("z", Code::KeyZ, Modifiers::META, Command), Some(Undo));
    // The value of the key can be in either case while Shift is down.
    for text in ["Z", "z"] {
        assert_eq!(
            step(
                text,
                Code::KeyZ,
                Modifiers::META | Modifiers::SHIFT,
                Command
            ),
            Some(Redo),
            "{text}"
        );
    }
}

#[test]
fn control_z_undoes_and_control_shift_z_redoes_on_the_other_platforms() {
    assert_eq!(
        step("z", Code::KeyZ, Modifiers::CONTROL, Control),
        Some(Undo)
    );
    for text in ["Z", "z"] {
        assert_eq!(
            step(
                text,
                Code::KeyZ,
                Modifiers::CONTROL | Modifiers::SHIFT,
                Control
            ),
            Some(Redo),
            "{text}"
        );
    }
}

#[test]
fn the_modifier_of_the_other_platform_is_no_shortcut() {
    assert_eq!(step("z", Code::KeyZ, Modifiers::CONTROL, Command), None);
    assert_eq!(step("z", Code::KeyZ, Modifiers::META, Control), None);
}

#[test]
fn one_more_modifier_makes_no_shortcut() {
    for (platform, own) in [(Command, Modifiers::META), (Control, Modifiers::CONTROL)] {
        for extra in [Modifiers::ALT, Modifiers::CONTROL | Modifiers::META] {
            for shift in [Modifiers::empty(), Modifiers::SHIFT] {
                let modifiers = own | extra | shift;
                assert_eq!(
                    step("z", Code::KeyZ, modifiers, platform),
                    None,
                    "{platform:?} {modifiers:?}"
                );
            }
        }
    }
}

#[test]
fn a_key_with_no_modifier_or_another_key_is_no_shortcut() {
    for platform in [Command, Control] {
        assert_eq!(step("z", Code::KeyZ, Modifiers::empty(), platform), None);
        assert_eq!(step("Z", Code::KeyZ, Modifiers::SHIFT, platform), None);
    }
    assert_eq!(step("y", Code::KeyY, Modifiers::META, Command), None);
    assert_eq!(step("x", Code::KeyX, Modifiers::CONTROL, Control), None);
    assert_eq!(
        shortcut_of(&Key::Enter, Code::Enter, Modifiers::META, Command),
        None
    );
}

#[test]
fn the_letter_counts_and_not_the_place_of_the_key() {
    // AZERTY: the letter z is on the key that QWERTY calls W.
    assert_eq!(step("z", Code::KeyW, Modifiers::META, Command), Some(Undo));
    // QWERTZ: the key that QWERTY calls Z types the letter y.
    assert_eq!(step("y", Code::KeyZ, Modifiers::META, Command), None);
}

#[test]
fn a_layout_with_no_latin_letters_uses_the_place_of_the_z_key() {
    // Russian: the key that QWERTY calls Z types the letter я.
    assert_eq!(
        step("я", Code::KeyZ, Modifiers::CONTROL, Control),
        Some(Undo)
    );
    assert_eq!(
        step("Я", Code::KeyZ, Modifiers::META | Modifiers::SHIFT, Command),
        Some(Redo)
    );
    assert_eq!(step("ч", Code::KeyX, Modifiers::META, Command), None);
}

#[test]
fn each_platform_names_its_keys() {
    assert_eq!(Command.keys(Undo), "\u{2318}Z");
    assert_eq!(Command.keys(Redo), "\u{21e7}\u{2318}Z");
    assert_eq!(Control.keys(Undo), "Ctrl+Z");
    assert_eq!(Control.keys(Redo), "Ctrl+Shift+Z");
}
