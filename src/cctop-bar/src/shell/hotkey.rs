//! The global hotkey: the map from a chord to the key of the `global-hotkey`
//! crate. The crate registers it through Carbon `RegisterEventHotKey`, which
//! needs no Accessibility permission and no Input Monitoring permission.

use global_hotkey::hotkey::HotKey;

use crate::chord::Chord;

/// The hotkey of `chord`, or `None` when its key has no counterpart.
pub fn hotkey_of(_chord: Chord) -> Option<HotKey> {
    None
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
