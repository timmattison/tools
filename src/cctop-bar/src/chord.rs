//! The global hotkey as text. A chord is a set of modifiers and one key, for
//! example `ctrl+alt+shift+cmd+backspace`.

use std::fmt;
use std::str::FromStr;

/// The chord that opens cctop when the person gives no other.
pub const DEFAULT_CHORD: &str = "ctrl+alt+shift+cmd+backspace";

/// The modifier keys of a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// The control key.
    pub control: bool,
    /// The option key, also called alt.
    pub option: bool,
    /// The shift key.
    pub shift: bool,
    /// The command key.
    pub command: bool,
}

/// The key of a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordKey {
    /// A letter, lower case, `a` to `z`.
    Letter(char),
    /// A digit key of the main block, 0 to 9.
    Digit(u8),
    /// A function key, 1 to 20.
    Function(u8),
    /// The backspace key, which Apple names delete.
    Backspace,
    /// The space bar.
    Space,
    /// The escape key.
    Escape,
    /// The return key.
    Return,
    /// The tab key.
    Tab,
    /// The up arrow.
    Up,
    /// The down arrow.
    Down,
    /// The left arrow.
    Left,
    /// The right arrow.
    Right,
}

/// A global hotkey: at least one modifier and one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    modifiers: Modifiers,
    key: ChordKey,
}

/// The reasons why a text is not a chord.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    /// The text is empty.
    #[error("the hotkey is empty; write modifiers and a key, for example ctrl+alt+k")]
    Empty,
    /// The text names no modifier.
    #[error(
        "the hotkey needs at least one modifier (ctrl, alt, shift, cmd), for example ctrl+alt+k"
    )]
    NoModifier,
    /// The text names no key.
    #[error("the hotkey needs a key after the modifiers, for example ctrl+alt+k")]
    NoKey,
    /// The text names two keys.
    #[error("the hotkey names two keys, {0} and {1}; use only one")]
    TwoKeys(String, String),
    /// A part is not a modifier or a key.
    #[error("{0:?} is not a modifier (ctrl, alt, shift, cmd) or a key (a-z, 0-9, f1-f20, backspace, space, escape, return, tab, up, down, left, right)")]
    Unknown(String),
}

impl Chord {
    /// Make a chord of `modifiers` and `key`.
    #[must_use]
    pub fn new(modifiers: Modifiers, key: ChordKey) -> Self {
        Self { modifiers, key }
    }

    /// The modifiers of the chord.
    #[must_use]
    pub fn modifiers(self) -> Modifiers {
        self.modifiers
    }

    /// The key of the chord.
    #[must_use]
    pub fn key(self) -> ChordKey {
        self.key
    }

    /// The Carbon modifier mask: `cmdKey` 0x0100, `shiftKey` 0x0200,
    /// `optionKey` 0x0800, `controlKey` 0x1000.
    #[must_use]
    pub fn carbon_modifiers(self) -> u32 {
        0
    }

    /// The Carbon virtual key code (`kVK_*`).
    #[must_use]
    pub fn virtual_key_code(self) -> u16 {
        0
    }

    /// The symbols that a Mac menu shows: the modifiers in the order
    /// control, option, shift, command, then the key.
    #[must_use]
    pub fn display(self) -> String {
        String::new()
    }
}

impl Default for Chord {
    fn default() -> Self {
        DEFAULT_CHORD
            .parse()
            .expect("the default chord is a valid chord")
    }
}

impl FromStr for Chord {
    type Err = ChordError;

    fn from_str(_text: &str) -> Result<Self, Self::Err> {
        Err(ChordError::Empty)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Chord {
        text.parse().unwrap()
    }

    fn mods(control: bool, option: bool, shift: bool, command: bool) -> Modifiers {
        Modifiers {
            control,
            option,
            shift,
            command,
        }
    }

    // Behavior 1: modifiers.
    #[test]
    fn each_modifier_name_and_alias_parses() {
        assert_eq!(parse("ctrl+a").modifiers(), mods(true, false, false, false));
        assert_eq!(
            parse("control+a").modifiers(),
            mods(true, false, false, false)
        );
        for name in ["alt", "opt", "option"] {
            assert_eq!(
                parse(&format!("{name}+a")).modifiers(),
                mods(false, true, false, false)
            );
        }
        assert_eq!(
            parse("shift+a").modifiers(),
            mods(false, false, true, false)
        );
        assert_eq!(parse("cmd+a").modifiers(), mods(false, false, false, true));
        assert_eq!(
            parse("command+a").modifiers(),
            mods(false, false, false, true)
        );
    }

    #[test]
    fn the_default_chord_has_four_modifiers_and_backspace() {
        let chord = Chord::default();
        assert_eq!(chord.modifiers(), mods(true, true, true, true));
        assert_eq!(chord.key(), ChordKey::Backspace);
    }

    #[test]
    fn delete_is_backspace() {
        assert_eq!(parse("cmd+delete"), parse("cmd+backspace"));
    }

    #[test]
    fn case_and_spaces_do_not_matter() {
        assert_eq!(parse("  CTRL + Alt+ K "), parse("ctrl+alt+k"));
        assert_eq!(parse("CMD+F12"), parse("cmd+f12"));
    }

    #[test]
    fn a_repeated_modifier_counts_once() {
        assert_eq!(parse("cmd+command+k"), parse("cmd+k"));
    }

    // Behavior 2: keys.
    #[test]
    fn each_key_name_parses() {
        assert_eq!(parse("cmd+a").key(), ChordKey::Letter('a'));
        assert_eq!(parse("cmd+Z").key(), ChordKey::Letter('z'));
        assert_eq!(parse("cmd+0").key(), ChordKey::Digit(0));
        assert_eq!(parse("cmd+9").key(), ChordKey::Digit(9));
        assert_eq!(parse("cmd+f1").key(), ChordKey::Function(1));
        assert_eq!(parse("cmd+f20").key(), ChordKey::Function(20));
        assert_eq!(parse("cmd+space").key(), ChordKey::Space);
        assert_eq!(parse("cmd+escape").key(), ChordKey::Escape);
        assert_eq!(parse("cmd+return").key(), ChordKey::Return);
        assert_eq!(parse("cmd+tab").key(), ChordKey::Tab);
        assert_eq!(parse("cmd+up").key(), ChordKey::Up);
        assert_eq!(parse("cmd+down").key(), ChordKey::Down);
        assert_eq!(parse("cmd+left").key(), ChordKey::Left);
        assert_eq!(parse("cmd+right").key(), ChordKey::Right);
    }

    #[test]
    fn a_bad_key_is_unknown() {
        for bad in ["f0", "f21", "é", "日本語", "🎉"] {
            assert_eq!(
                format!("ctrl+{bad}").parse::<Chord>(),
                Err(ChordError::Unknown(bad.to_string())),
                "{bad}"
            );
        }
    }

    #[test]
    fn an_unknown_error_names_the_part_and_the_valid_names() {
        let message = "ctrl+日本語".parse::<Chord>().unwrap_err().to_string();
        assert!(message.contains("日本語"));
        assert!(message.contains("backspace"));
    }

    // Behavior 3: refusals.
    #[test]
    fn the_refusals() {
        assert_eq!("".parse::<Chord>(), Err(ChordError::Empty));
        assert_eq!("   ".parse::<Chord>(), Err(ChordError::Empty));
        assert_eq!("backspace".parse::<Chord>(), Err(ChordError::NoModifier));
        assert_eq!("ctrl+shift".parse::<Chord>(), Err(ChordError::NoKey));
        assert_eq!(
            "ctrl+a+b".parse::<Chord>(),
            Err(ChordError::TwoKeys("a".to_string(), "b".to_string()))
        );
    }

    // Behavior 4: Carbon codes.
    #[test]
    fn carbon_values_follow_events_h() {
        let default = Chord::default();
        assert_eq!(default.carbon_modifiers(), 0x1B00);
        assert_eq!(default.virtual_key_code(), 0x33);
        assert_eq!(parse("cmd+a").carbon_modifiers(), 0x0100);
        assert_eq!(parse("shift+a").carbon_modifiers(), 0x0200);
        assert_eq!(parse("alt+a").carbon_modifiers(), 0x0800);
        assert_eq!(parse("ctrl+a").carbon_modifiers(), 0x1000);
        let codes = [
            ("a", 0x00),
            ("0", 0x1D),
            ("f1", 0x7A),
            ("f20", 0x5A),
            ("left", 0x7B),
            ("right", 0x7C),
            ("down", 0x7D),
            ("up", 0x7E),
            ("return", 0x24),
            ("tab", 0x30),
            ("space", 0x31),
            ("escape", 0x35),
            ("m", 0x2E),
            ("b", 0x0B),
            ("6", 0x16),
            ("f17", 0x40),
        ];
        for (key, code) in codes {
            assert_eq!(
                parse(&format!("cmd+{key}")).virtual_key_code(),
                code,
                "{key}"
            );
        }
    }

    // Behavior 5: display.
    #[test]
    fn display_shows_mac_symbols() {
        assert_eq!(Chord::default().display(), "⌃⌥⇧⌘⌫");
        assert_eq!(parse("cmd+k").display(), "⌘K");
        assert_eq!(parse("cmd+f12").display(), "⌘F12");
        assert_eq!(parse("ctrl+5").display(), "⌃5");
        assert_eq!(parse("cmd+space").display(), "⌘Space");
        assert_eq!(parse("cmd+escape").display(), "⌘⎋");
        assert_eq!(parse("cmd+return").display(), "⌘↩");
        assert_eq!(parse("cmd+tab").display(), "⌘⇥");
        assert_eq!(parse("cmd+up").display(), "⌘↑");
        assert_eq!(parse("cmd+down").display(), "⌘↓");
        assert_eq!(parse("cmd+left").display(), "⌘←");
        assert_eq!(parse("cmd+right").display(), "⌘→");
    }

    // Behavior 6: round trip.
    #[test]
    fn text_round_trips_for_every_key_and_several_modifier_sets() {
        let mut keys = vec![
            ChordKey::Backspace,
            ChordKey::Space,
            ChordKey::Escape,
            ChordKey::Return,
            ChordKey::Tab,
            ChordKey::Up,
            ChordKey::Down,
            ChordKey::Left,
            ChordKey::Right,
        ];
        keys.extend(('a'..='z').map(ChordKey::Letter));
        keys.extend((0..=9).map(ChordKey::Digit));
        keys.extend((1..=20).map(ChordKey::Function));
        let sets = [
            mods(true, false, false, false),
            mods(false, true, false, true),
            mods(true, true, true, true),
            mods(false, false, true, false),
        ];
        for key in keys {
            for set in sets {
                let chord = Chord::new(set, key);
                assert_eq!(chord.to_string().parse::<Chord>(), Ok(chord), "{chord}");
            }
        }
    }

    #[test]
    fn the_default_chord_prints_as_the_constant() {
        assert_eq!(Chord::default().to_string(), DEFAULT_CHORD);
        assert_eq!(parse("cmd+ctrl+k").to_string(), "ctrl+cmd+k");
    }
}
