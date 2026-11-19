//! The global hotkey as text. A chord is a set of modifiers and one key, for
//! example `ctrl+alt+shift+cmd+backspace`.

use std::fmt::{self, Write as _};
use std::str::FromStr;

/// The chord that opens cctop when the person gives no other.
pub const DEFAULT_CHORD: &str = "ctrl+alt+shift+cmd+backspace";

/// The modifier keys of a chord.
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one physical modifier key"
)]
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
        let Modifiers {
            control,
            option,
            shift,
            command,
        } = self.modifiers;
        [
            (command, CARBON_COMMAND),
            (shift, CARBON_SHIFT),
            (option, CARBON_OPTION),
            (control, CARBON_CONTROL),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .fold(0, |mask, (_, bit)| mask | bit)
    }

    /// The Carbon virtual key code (`kVK_*`), from `HIToolbox/Events.h`.
    #[must_use]
    pub fn virtual_key_code(self) -> u16 {
        match self.key {
            ChordKey::Letter(letter) => letter_code(letter),
            ChordKey::Digit(digit) => digit_code(digit),
            ChordKey::Function(number) => function_code(number),
            ChordKey::Backspace => 0x33,
            ChordKey::Space => 0x31,
            ChordKey::Escape => 0x35,
            ChordKey::Return => 0x24,
            ChordKey::Tab => 0x30,
            ChordKey::Up => 0x7E,
            ChordKey::Down => 0x7D,
            ChordKey::Left => 0x7B,
            ChordKey::Right => 0x7C,
        }
    }

    /// The symbols that a Mac menu shows: the modifiers in the order
    /// control, option, shift, command, then the key.
    #[must_use]
    pub fn display(self) -> String {
        let Modifiers {
            control,
            option,
            shift,
            command,
        } = self.modifiers;
        let mut text = String::new();
        for (held, symbol) in [
            (control, '\u{2303}'),
            (option, '\u{2325}'),
            (shift, '\u{21E7}'),
            (command, '\u{2318}'),
        ] {
            if held {
                text.push(symbol);
            }
        }
        match self.key {
            ChordKey::Letter(letter) => text.push(letter.to_ascii_uppercase()),
            ChordKey::Digit(digit) => text.push_str(&digit.to_string()),
            ChordKey::Function(number) => {
                let _ = write!(text, "F{number}");
            }
            ChordKey::Backspace => text.push('\u{232B}'),
            ChordKey::Space => text.push_str("Space"),
            ChordKey::Escape => text.push('\u{238B}'),
            ChordKey::Return => text.push('\u{21A9}'),
            ChordKey::Tab => text.push('\u{21E5}'),
            ChordKey::Up => text.push('\u{2191}'),
            ChordKey::Down => text.push('\u{2193}'),
            ChordKey::Left => text.push('\u{2190}'),
            ChordKey::Right => text.push('\u{2192}'),
        }
        text
    }
}

/// The Carbon mask bit of the command key.
const CARBON_COMMAND: u32 = 0x0100;
/// The Carbon mask bit of the shift key.
const CARBON_SHIFT: u32 = 0x0200;
/// The Carbon mask bit of the option key.
const CARBON_OPTION: u32 = 0x0800;
/// The Carbon mask bit of the control key.
const CARBON_CONTROL: u32 = 0x1000;

/// The key codes of `kVK_ANSI_A` to `kVK_ANSI_Z`, indexed from `a`. The
/// values come from `HIToolbox/Events.h`.
const LETTER_CODES: [u16; 26] = [
    0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23,
    0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
];

/// The key codes of `kVK_ANSI_0` to `kVK_ANSI_9`, indexed from 0. The values
/// come from `HIToolbox/Events.h`.
const DIGIT_CODES: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];

/// The key codes of `kVK_F1` to `kVK_F20`, indexed from F1. The values come
/// from `HIToolbox/Events.h`.
const FUNCTION_CODES: [u16; 20] = [
    0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A,
    0x40, 0x4F, 0x50, 0x5A,
];

/// The key code of a lower case letter. The parser admits no other letter.
fn letter_code(letter: char) -> u16 {
    let index = (letter as u32).saturating_sub('a' as u32) as usize;
    LETTER_CODES.get(index).copied().unwrap_or(0)
}

/// The key code of a digit.
fn digit_code(digit: u8) -> u16 {
    DIGIT_CODES.get(usize::from(digit)).copied().unwrap_or(0)
}

/// The key code of a function key, numbered from 1.
fn function_code(number: u8) -> u16 {
    FUNCTION_CODES
        .get(usize::from(number).wrapping_sub(1))
        .copied()
        .unwrap_or(0)
}

/// What one part of a chord text names.
enum Part {
    Modifier(fn(&mut Modifiers)),
    Key(ChordKey),
}

/// Read one trimmed, lower case part.
fn read_part(part: &str) -> Option<Part> {
    let modifier = |set: fn(&mut Modifiers)| Some(Part::Modifier(set));
    match part {
        "ctrl" | "control" => modifier(|m| m.control = true),
        "alt" | "opt" | "option" => modifier(|m| m.option = true),
        "shift" => modifier(|m| m.shift = true),
        "cmd" | "command" => modifier(|m| m.command = true),
        "backspace" | "delete" => Some(Part::Key(ChordKey::Backspace)),
        "space" => Some(Part::Key(ChordKey::Space)),
        "escape" => Some(Part::Key(ChordKey::Escape)),
        "return" => Some(Part::Key(ChordKey::Return)),
        "tab" => Some(Part::Key(ChordKey::Tab)),
        "up" => Some(Part::Key(ChordKey::Up)),
        "down" => Some(Part::Key(ChordKey::Down)),
        "left" => Some(Part::Key(ChordKey::Left)),
        "right" => Some(Part::Key(ChordKey::Right)),
        _ => read_character_key(part).map(Part::Key),
    }
}

/// Read a letter, a digit, or a function key.
fn read_character_key(part: &str) -> Option<ChordKey> {
    let mut chars = part.chars();
    if let (Some(first), None) = (chars.next(), chars.next()) {
        if first.is_ascii_lowercase() {
            return Some(ChordKey::Letter(first));
        }
        if let Some(digit) = first.to_digit(10).filter(|_| first.is_ascii_digit()) {
            return u8::try_from(digit).ok().map(ChordKey::Digit);
        }
        return None;
    }
    let number: u8 = part.strip_prefix('f')?.parse().ok()?;
    let plain = part.len() == 1 + number.to_string().len();
    (plain && (1..=20).contains(&number)).then_some(ChordKey::Function(number))
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

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.trim().is_empty() {
            return Err(ChordError::Empty);
        }
        let mut modifiers = Modifiers::default();
        let mut key: Option<(String, ChordKey)> = None;
        let mut any_modifier = false;
        for raw in text.split('+') {
            let part = raw.trim();
            let lower = part.to_lowercase();
            match read_part(&lower) {
                Some(Part::Modifier(set)) => {
                    set(&mut modifiers);
                    any_modifier = true;
                }
                Some(Part::Key(found)) => {
                    if let Some((first, _)) = key {
                        return Err(ChordError::TwoKeys(first, lower));
                    }
                    key = Some((lower, found));
                }
                None => return Err(ChordError::Unknown(part.to_string())),
            }
        }
        match (any_modifier, key) {
            (false, _) => Err(ChordError::NoModifier),
            (true, None) => Err(ChordError::NoKey),
            (true, Some((_, key))) => Ok(Self { modifiers, key }),
        }
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Modifiers {
            control,
            option,
            shift,
            command,
        } = self.modifiers;
        for (held, name) in [
            (control, "ctrl+"),
            (option, "alt+"),
            (shift, "shift+"),
            (command, "cmd+"),
        ] {
            if held {
                f.write_str(name)?;
            }
        }
        match self.key {
            ChordKey::Letter(letter) => write!(f, "{letter}"),
            ChordKey::Digit(digit) => write!(f, "{digit}"),
            ChordKey::Function(number) => write!(f, "f{number}"),
            ChordKey::Backspace => f.write_str("backspace"),
            ChordKey::Space => f.write_str("space"),
            ChordKey::Escape => f.write_str("escape"),
            ChordKey::Return => f.write_str("return"),
            ChordKey::Tab => f.write_str("tab"),
            ChordKey::Up => f.write_str("up"),
            ChordKey::Down => f.write_str("down"),
            ChordKey::Left => f.write_str("left"),
            ChordKey::Right => f.write_str("right"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Chord {
        text.parse().unwrap()
    }

    fn mods([control, option, shift, command]: [bool; 4]) -> Modifiers {
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
        assert_eq!(
            parse("ctrl+a").modifiers(),
            mods([true, false, false, false])
        );
        assert_eq!(
            parse("control+a").modifiers(),
            mods([true, false, false, false])
        );
        for name in ["alt", "opt", "option"] {
            assert_eq!(
                parse(&format!("{name}+a")).modifiers(),
                mods([false, true, false, false])
            );
        }
        assert_eq!(
            parse("shift+a").modifiers(),
            mods([false, false, true, false])
        );
        assert_eq!(
            parse("cmd+a").modifiers(),
            mods([false, false, false, true])
        );
        assert_eq!(
            parse("command+a").modifiers(),
            mods([false, false, false, true])
        );
    }

    #[test]
    fn the_default_chord_has_four_modifiers_and_backspace() {
        let chord = Chord::default();
        assert_eq!(chord.modifiers(), mods([true, true, true, true]));
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

    #[test]
    fn every_letter_digit_and_function_key_has_its_events_h_code() {
        let letters = [
            ('a', 0x00),
            ('s', 0x01),
            ('d', 0x02),
            ('f', 0x03),
            ('h', 0x04),
            ('g', 0x05),
            ('z', 0x06),
            ('x', 0x07),
            ('c', 0x08),
            ('v', 0x09),
            ('b', 0x0B),
            ('q', 0x0C),
            ('w', 0x0D),
            ('e', 0x0E),
            ('r', 0x0F),
            ('y', 0x10),
            ('t', 0x11),
            ('o', 0x1F),
            ('u', 0x20),
            ('i', 0x22),
            ('p', 0x23),
            ('l', 0x25),
            ('j', 0x26),
            ('k', 0x28),
            ('n', 0x2D),
            ('m', 0x2E),
        ];
        assert_eq!(letters.len(), 26);
        for (letter, code) in letters {
            let chord = Chord::new(mods([false, false, false, true]), ChordKey::Letter(letter));
            assert_eq!(chord.virtual_key_code(), code, "{letter}");
        }
        let digits = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
        for (digit, code) in (0_u8..).zip(digits) {
            let chord = Chord::new(mods([false, false, false, true]), ChordKey::Digit(digit));
            assert_eq!(chord.virtual_key_code(), code, "{digit}");
        }
        let functions = [
            0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B,
            0x71, 0x6A, 0x40, 0x4F, 0x50, 0x5A,
        ];
        for (number, code) in (1_u8..).zip(functions) {
            let chord = Chord::new(
                mods([false, false, false, true]),
                ChordKey::Function(number),
            );
            assert_eq!(chord.virtual_key_code(), code, "F{number}");
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
            mods([true, false, false, false]),
            mods([false, true, false, true]),
            mods([true, true, true, true]),
            mods([false, false, true, false]),
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
