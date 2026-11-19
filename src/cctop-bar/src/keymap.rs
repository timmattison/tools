//! An `AppKit` key event as a crossterm key, so `App::handle_key` gets the
//! same keys in the panel as in the terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The shift bit of `NSEventModifierFlags`.
pub const SHIFT_FLAG: u64 = 1 << 17;
/// The control bit of `NSEventModifierFlags`.
pub const CONTROL_FLAG: u64 = 1 << 18;
/// The option bit of `NSEventModifierFlags`.
pub const OPTION_FLAG: u64 = 1 << 19;
/// The command bit of `NSEventModifierFlags`.
pub const COMMAND_FLAG: u64 = 1 << 20;

/// The key code of the up arrow.
pub const KEY_CODE_UP: u16 = 126;
/// The key code of the down arrow.
pub const KEY_CODE_DOWN: u16 = 125;
/// The key code of the left arrow.
pub const KEY_CODE_LEFT: u16 = 123;
/// The key code of the right arrow.
pub const KEY_CODE_RIGHT: u16 = 124;
/// The key code of Page Up.
pub const KEY_CODE_PAGE_UP: u16 = 116;
/// The key code of Page Down.
pub const KEY_CODE_PAGE_DOWN: u16 = 121;
/// The key code of Home.
pub const KEY_CODE_HOME: u16 = 115;
/// The key code of End.
pub const KEY_CODE_END: u16 = 119;
/// The key code of Return.
pub const KEY_CODE_RETURN: u16 = 36;
/// The key code of Enter on the keypad.
pub const KEY_CODE_KEYPAD_ENTER: u16 = 76;
/// The key code of Tab.
pub const KEY_CODE_TAB: u16 = 48;
/// The key code of Backspace, which Apple names Delete.
pub const KEY_CODE_BACKSPACE: u16 = 51;
/// The key code of Escape.
pub const KEY_CODE_ESCAPE: u16 = 53;

/// What the panel does with a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelKey {
    /// Give this key to the app.
    Key(KeyEvent),
    /// Hide the panel.
    Hide,
    /// Do nothing.
    Ignore,
}

/// Map a key event of `AppKit`. `key_code` is `NSEvent.keyCode`, `characters`
/// is `NSEvent.charactersIgnoringModifiers`, and `flags` is
/// `NSEvent.modifierFlags` as raw bits.
#[must_use]
pub fn map_key(key_code: u16, characters: Option<&str>, flags: u64) -> PanelKey {
    let _ = (
        key_code,
        characters,
        flags,
        KeyCode::Enter,
        KeyModifiers::NONE,
    );
    PanelKey::Ignore
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key that `map_key` gives, or a failure when it gives none.
    fn key(key_code: u16, characters: Option<&str>, flags: u64) -> KeyEvent {
        match map_key(key_code, characters, flags) {
            PanelKey::Key(event) => event,
            other => panic!("expected a key, got {other:?}"),
        }
    }

    #[test]
    fn the_keys_of_the_cctop_table_map_to_crossterm_keys() {
        let plain = |code: KeyCode| KeyEvent::new(code, KeyModifiers::NONE);
        assert_eq!(key(38, Some("j"), 0), plain(KeyCode::Char('j')));
        assert_eq!(key(40, Some("k"), 0), plain(KeyCode::Char('k')));
        assert_eq!(key(KEY_CODE_UP, Some("\u{f700}"), 0), plain(KeyCode::Up));
        assert_eq!(
            key(KEY_CODE_DOWN, Some("\u{f701}"), 0),
            plain(KeyCode::Down)
        );
        assert_eq!(
            key(KEY_CODE_PAGE_UP, Some("\u{f72c}"), 0),
            plain(KeyCode::PageUp)
        );
        assert_eq!(
            key(KEY_CODE_PAGE_DOWN, Some("\u{f72d}"), 0),
            plain(KeyCode::PageDown)
        );
        assert_eq!(
            key(KEY_CODE_HOME, Some("\u{f729}"), 0),
            plain(KeyCode::Home)
        );
        assert_eq!(key(KEY_CODE_END, Some("\u{f72b}"), 0), plain(KeyCode::End));
        assert_eq!(key(KEY_CODE_RETURN, Some("\r"), 0), plain(KeyCode::Enter));
        assert_eq!(
            key(KEY_CODE_KEYPAD_ENTER, Some("\u{3}"), 0),
            plain(KeyCode::Enter)
        );
        assert_eq!(key(45, Some("n"), 0), plain(KeyCode::Char('n')));
        assert_eq!(key(4, Some("h"), 0), plain(KeyCode::Char('h')));
        assert_eq!(key(12, Some("q"), 0), plain(KeyCode::Char('q')));
        assert_eq!(key(KEY_CODE_TAB, Some("\t"), 0), plain(KeyCode::Tab));
        assert_eq!(
            key(KEY_CODE_BACKSPACE, Some("\u{7f}"), 0),
            plain(KeyCode::Backspace)
        );
        assert_eq!(key(KEY_CODE_LEFT, None, 0), plain(KeyCode::Left));
        assert_eq!(key(KEY_CODE_RIGHT, None, 0), plain(KeyCode::Right));
    }

    #[test]
    fn shift_keeps_the_shift_modifier_of_a_capital_h() {
        assert_eq!(
            key(4, Some("H"), SHIFT_FLAG),
            KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT)
        );
    }

    #[test]
    fn control_c_is_a_c_with_the_control_modifier() {
        assert_eq!(
            key(8, Some("c"), CONTROL_FLAG),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        );
    }
}
