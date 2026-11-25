//! The global hotkey: the map from a chord to the key of the `global-hotkey`
//! crate. The crate registers it through Carbon `RegisterEventHotKey`, which
//! needs no Accessibility permission and no Input Monitoring permission.

use global_hotkey::hotkey::{Code, HotKey, Modifiers};

use crate::chord::{Chord, ChordKey};

/// The key codes of the letters, in the order `a` to `z`.
const LETTERS: [Code; 26] = [
    Code::KeyA,
    Code::KeyB,
    Code::KeyC,
    Code::KeyD,
    Code::KeyE,
    Code::KeyF,
    Code::KeyG,
    Code::KeyH,
    Code::KeyI,
    Code::KeyJ,
    Code::KeyK,
    Code::KeyL,
    Code::KeyM,
    Code::KeyN,
    Code::KeyO,
    Code::KeyP,
    Code::KeyQ,
    Code::KeyR,
    Code::KeyS,
    Code::KeyT,
    Code::KeyU,
    Code::KeyV,
    Code::KeyW,
    Code::KeyX,
    Code::KeyY,
    Code::KeyZ,
];

/// The key codes of the digits, in the order 0 to 9.
const DIGITS: [Code; 10] = [
    Code::Digit0,
    Code::Digit1,
    Code::Digit2,
    Code::Digit3,
    Code::Digit4,
    Code::Digit5,
    Code::Digit6,
    Code::Digit7,
    Code::Digit8,
    Code::Digit9,
];

/// The key codes of the function keys, in the order F1 to F20.
const FUNCTION_KEYS: [Code; 20] = [
    Code::F1,
    Code::F2,
    Code::F3,
    Code::F4,
    Code::F5,
    Code::F6,
    Code::F7,
    Code::F8,
    Code::F9,
    Code::F10,
    Code::F11,
    Code::F12,
    Code::F13,
    Code::F14,
    Code::F15,
    Code::F16,
    Code::F17,
    Code::F18,
    Code::F19,
    Code::F20,
];

/// The key code of `key`, or `None` when the key is outside the ranges that
/// the chord parser allows.
fn code_of(key: ChordKey) -> Option<Code> {
    match key {
        ChordKey::Letter(letter) => {
            let index = (letter as usize).checked_sub('a' as usize)?;
            LETTERS.get(index).copied()
        }
        ChordKey::Digit(digit) => DIGITS.get(usize::from(digit)).copied(),
        ChordKey::Function(number) => FUNCTION_KEYS
            .get(usize::from(number).checked_sub(1)?)
            .copied(),
        ChordKey::Backspace => Some(Code::Backspace),
        ChordKey::Space => Some(Code::Space),
        ChordKey::Escape => Some(Code::Escape),
        ChordKey::Return => Some(Code::Enter),
        ChordKey::Tab => Some(Code::Tab),
        ChordKey::Up => Some(Code::ArrowUp),
        ChordKey::Down => Some(Code::ArrowDown),
        ChordKey::Left => Some(Code::ArrowLeft),
        ChordKey::Right => Some(Code::ArrowRight),
    }
}

/// The hotkey of `chord`, or `None` when its key has no counterpart.
pub fn hotkey_of(chord: Chord) -> Option<HotKey> {
    let held = chord.modifiers();
    let mut modifiers = Modifiers::empty();
    for (is_held, flag) in [
        (held.control, Modifiers::CONTROL),
        (held.option, Modifiers::ALT),
        (held.shift, Modifiers::SHIFT),
        (held.command, Modifiers::META),
    ] {
        if is_held {
            modifiers |= flag;
        }
    }
    Some(HotKey::new(Some(modifiers), code_of(chord.key())?))
}

#[cfg(test)]
mod tests {
    use global_hotkey::hotkey::{Code, Modifiers};

    use super::*;

    fn hotkey(text: &str) -> HotKey {
        hotkey_of(text.parse().expect("the chord parses")).expect("the key has a counterpart")
    }

    #[test]
    fn the_default_chord_is_four_modifiers_and_backspace() {
        let all = Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT | Modifiers::META;
        assert_eq!(
            hotkey("ctrl+alt+shift+cmd+backspace"),
            HotKey::new(Some(all), Code::Backspace)
        );
    }

    #[test]
    fn each_modifier_maps_to_its_flag() {
        assert_eq!(
            hotkey("ctrl+k"),
            HotKey::new(Some(Modifiers::CONTROL), Code::KeyK)
        );
        assert_eq!(
            hotkey("alt+k"),
            HotKey::new(Some(Modifiers::ALT), Code::KeyK)
        );
        assert_eq!(
            hotkey("shift+k"),
            HotKey::new(Some(Modifiers::SHIFT), Code::KeyK)
        );
        assert_eq!(
            hotkey("cmd+k"),
            HotKey::new(Some(Modifiers::META), Code::KeyK)
        );
    }

    #[test]
    fn letters_digits_function_keys_and_named_keys_map_to_their_codes() {
        let cases = [
            ("ctrl+a", Code::KeyA),
            ("ctrl+z", Code::KeyZ),
            ("ctrl+0", Code::Digit0),
            ("ctrl+9", Code::Digit9),
            ("ctrl+f1", Code::F1),
            ("ctrl+f12", Code::F12),
            ("ctrl+f20", Code::F20),
            ("ctrl+space", Code::Space),
            ("ctrl+escape", Code::Escape),
            ("ctrl+return", Code::Enter),
            ("ctrl+tab", Code::Tab),
            ("ctrl+up", Code::ArrowUp),
            ("ctrl+down", Code::ArrowDown),
            ("ctrl+left", Code::ArrowLeft),
            ("ctrl+right", Code::ArrowRight),
        ];
        for (text, code) in cases {
            assert_eq!(
                hotkey(text),
                HotKey::new(Some(Modifiers::CONTROL), code),
                "{text}"
            );
        }
    }

    #[test]
    fn a_key_with_no_counterpart_gives_none() {
        use crate::chord::{ChordKey, Modifiers as ChordModifiers};
        let modifiers = ChordModifiers {
            control: true,
            ..ChordModifiers::default()
        };
        assert!(hotkey_of(Chord::new(modifiers, ChordKey::Letter('é'))).is_none());
        assert!(hotkey_of(Chord::new(modifiers, ChordKey::Function(21))).is_none());
        assert!(hotkey_of(Chord::new(modifiers, ChordKey::Digit(10))).is_none());
    }
}
