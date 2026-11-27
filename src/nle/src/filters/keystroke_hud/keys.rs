//! The parser that turns the data of one input event into keys.

/// One key in the data of an input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Key {
    /// Printable text, as the user typed it.
    Text(String),
    /// A key that prints nothing, or a key with a modifier, by its label, for
    /// example `Enter`, `Ctrl+C`, `↑`, or `Alt+.`.
    Special(String),
}

/// The keys in the data of one input event, in order. Each run of printable
/// characters is one `Key::Text`. A byte sequence that is not a key (a mouse
/// report, an answer of the terminal) gives no key.
pub(crate) fn parse(data: &str) -> Vec<Key> {
    let mut parser = Parser {
        chars: data.chars().collect(),
        pos: 0,
        keys: Vec::new(),
    };
    parser.run();
    parser.keys
}

/// Reads the input events of one region in order.
pub(crate) struct KeyReader {}

impl KeyReader {
    /// A reader with no open paste and no unfinished sequence.
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// The keys that the input event at `time` with `data` completes, in
    /// order, each with its time.
    pub(crate) fn feed(&mut self, time: f64, data: &str) -> Vec<(f64, Key)> {
        parse(data).into_iter().map(|key| (time, key)).collect()
    }

    /// The keys of a sequence that is still unfinished at the end of the input.
    pub(crate) fn finish(&mut self) -> Vec<(f64, Key)> {
        Vec::new()
    }
}

const ESC: char = '\x1b';
const BEL: char = '\x07';
const STRING_TERMINATOR: char = '\\';

const ENTER: &str = "Enter";
const TAB: &str = "Tab";
const BACKSPACE: &str = "Backspace";
const SPACE: &str = "Space";
const ESCAPE_LABEL: &str = "Esc";
const PASTE: &str = "Paste";

/// The end of a bracketed paste: `ESC [ 201 ~`.
const PASTE_END: [char; 6] = [ESC, '[', '2', '0', '1', '~'];
/// The code of the CSI key that starts a bracketed paste.
const PASTE_START_CODE: u32 = 200;
/// The code of the CSI key of the xterm modifyOtherKeys form,
/// `CSI 27 ; mod ; code ~`. Field 2 holds the code of the key.
const MODIFY_OTHER_KEYS_CODE: u32 = 27;
/// The event type of a key release in the kitty keyboard protocol.
const EVENT_RELEASE: u32 = 3;
/// The range of the private use code points. The kitty protocol puts lock
/// keys, modifier keys, and keypad keys there.
const PRIVATE_USE_CODES: std::ops::RangeInclusive<u32> = 57_344..=63_743;

/// The modifier keys of a key.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Mods {
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
}

impl Mods {
    const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        super_key: false,
    };

    /// Read the modifier value of a CSI sequence. The bits are the value minus
    /// 1: Shift 1, Alt 2, Ctrl 4, Super 8. The other bits have no label.
    fn from_value(value: u32) -> Self {
        let bits = value.saturating_sub(1);
        Self {
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            ctrl: bits & 4 != 0,
            super_key: bits & 8 != 0,
        }
    }

    /// The same modifiers with Alt added.
    fn with_alt(self) -> Self {
        Self { alt: true, ..self }
    }

    /// The same modifiers with Shift added.
    fn with_shift(self) -> Self {
        Self {
            shift: true,
            ..self
        }
    }

    /// True when no modifier is set.
    fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// The label of a key with these modifiers. The order of the prefixes is
    /// Ctrl, Alt, Shift, Super.
    fn label(self, base: &str) -> String {
        let mut label = String::new();
        for (set, prefix) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.super_key, "Super+"),
        ] {
            if set {
                label.push_str(prefix);
            }
        }
        label.push_str(base);
        label
    }
}

/// The label of the character `c` with the modifiers `mods`, of which there
/// is at least one. An ASCII letter shows in upper case, as in `Ctrl+C`, and
/// an upper-case letter adds Shift. Thus each encoding of a key gives one
/// label: `ESC F` and `CSI 102;4u` both give `Alt+Shift+F`.
fn modified_label(mods: Mods, c: char) -> String {
    let mods = if c.is_ascii_uppercase() {
        mods.with_shift()
    } else {
        mods
    };
    mods.label(c.to_ascii_uppercase().encode_utf8(&mut [0; 4]))
}

/// The key of a control character that is not part of an escape sequence, as
/// its modifiers and its base label. `None` when the character gives no key.
fn control_key(c: char) -> Option<(Mods, String)> {
    let ctrl = Mods {
        ctrl: true,
        ..Mods::NONE
    };
    let code = u32::from(c);
    let key = match c {
        '\r' | '\n' => (Mods::NONE, ENTER.to_string()),
        '\t' => (Mods::NONE, TAB.to_string()),
        '\x7f' | '\x08' => (Mods::NONE, BACKSPACE.to_string()),
        '\x00' => (ctrl, SPACE.to_string()),
        '\x01'..='\x1a' => {
            let letter = char::from_u32(u32::from('A') + code - 1)?;
            (ctrl, letter.to_string())
        }
        '\x1c' => (ctrl, "\\".to_string()),
        '\x1d' => (ctrl, "]".to_string()),
        '\x1e' => (ctrl, "^".to_string()),
        '\x1f' => (ctrl, "_".to_string()),
        _ => return None,
    };
    Some(key)
}

/// The label of a cursor key or a function key with a letter as final
/// character, in a CSI sequence.
fn csi_letter_label(final_char: char) -> Option<&'static str> {
    let label = match final_char {
        'A' => "↑",
        'B' => "↓",
        'C' => "→",
        'D' => "←",
        'H' => "Home",
        'F' => "End",
        'P' => "F1",
        'Q' => "F2",
        'S' => "F4",
        'Z' => TAB,
        _ => return None,
    };
    Some(label)
}

/// The label of a CSI key with `~` as final character, by its code.
fn csi_tilde_label(code: u32) -> Option<&'static str> {
    let label = match code {
        1 | 7 => "Home",
        2 => "Insert",
        3 => "Delete",
        4 | 8 => "End",
        5 => "PgUp",
        6 => "PgDn",
        11 => "F1",
        12 => "F2",
        13 => "F3",
        14 => "F4",
        15 => "F5",
        17 => "F6",
        18 => "F7",
        19 => "F8",
        20 => "F9",
        21 => "F10",
        23 => "F11",
        24 => "F12",
        _ => return None,
    };
    Some(label)
}

/// What an SS3 sequence gives.
enum Ss3Key {
    Special(&'static str),
    Text(char),
}

/// The key of the character after `ESC O`.
fn ss3_key(c: char) -> Option<Ss3Key> {
    let key = match c {
        'A' => Ss3Key::Special("↑"),
        'B' => Ss3Key::Special("↓"),
        'C' => Ss3Key::Special("→"),
        'D' => Ss3Key::Special("←"),
        'H' => Ss3Key::Special("Home"),
        'F' => Ss3Key::Special("End"),
        'P' => Ss3Key::Special("F1"),
        'Q' => Ss3Key::Special("F2"),
        'R' => Ss3Key::Special("F3"),
        'S' => Ss3Key::Special("F4"),
        'M' => Ss3Key::Special(ENTER),
        'j' => Ss3Key::Text('*'),
        'k' => Ss3Key::Text('+'),
        'l' => Ss3Key::Text(','),
        'm' => Ss3Key::Text('-'),
        'n' => Ss3Key::Text('.'),
        'o' => Ss3Key::Text('/'),
        'X' => Ss3Key::Text('='),
        'p'..='y' => Ss3Key::Text(char::from_u32(
            u32::from('0') + u32::from(c) - u32::from('p'),
        )?),
        _ => return None,
    };
    Some(key)
}

/// The numbers in the parameters of a CSI sequence. Field 0 is the code,
/// field 1 holds the modifier value and the event type.
struct CsiParams {
    fields: Vec<Vec<Option<u32>>>,
}

impl CsiParams {
    /// Split the parameters by `;` into fields and by `:` into parts. An empty
    /// part, and a number that does not fit in `u32`, is absent.
    fn new(params: &str) -> Self {
        let fields = params
            .split(';')
            .map(|field| field.split(':').map(|part| part.parse().ok()).collect())
            .collect();
        Self { fields }
    }

    /// Part `part` of field `field`, when it is present.
    fn get(&self, field: usize, part: usize) -> Option<u32> {
        self.fields.get(field)?.get(part).copied().flatten()
    }

    /// The code of the key. The default is 1.
    fn code(&self) -> u32 {
        self.get(0, 0).unwrap_or(1)
    }

    /// The modifiers of the key.
    fn mods(&self) -> Mods {
        Mods::from_value(self.get(1, 0).unwrap_or(1))
    }

    /// The code of the key in the xterm modifyOtherKeys form, when it is
    /// present.
    fn other_key_code(&self) -> Option<u32> {
        self.get(2, 0)
    }

    /// The event type. The default is 1, a key press.
    fn event_type(&self) -> u32 {
        self.get(1, 1).unwrap_or(1)
    }
}

/// The state of one parse.
struct Parser {
    chars: Vec<char>,
    pos: usize,
    keys: Vec<Key>,
}

impl Parser {
    /// Parse all of the data.
    fn run(&mut self) {
        while let Some(c) = self.next_char() {
            match c {
                ESC => self.escape(),
                '\r' => {
                    if self.peek() == Some('\n') {
                        self.pos += 1;
                    }
                    self.push_special(ENTER.to_string());
                }
                c if c.is_control() => self.push_control(c, Mods::NONE),
                c => self.push_text(c.encode_utf8(&mut [0; 4])),
            }
        }
    }

    /// The next character, without a move.
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// The next character, with a move.
    fn next_char(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    /// Add text. It joins the text before it, when that is also text.
    fn push_text(&mut self, text: &str) {
        if let Some(Key::Text(last)) = self.keys.last_mut() {
            last.push_str(text);
        } else {
            self.keys.push(Key::Text(text.to_string()));
        }
    }

    /// Add a key by its label.
    fn push_special(&mut self, label: String) {
        self.keys.push(Key::Special(label));
    }

    /// Add the key of a control character, with extra modifiers. Add nothing
    /// when the character gives no key.
    fn push_control(&mut self, c: char, extra: Mods) {
        if let Some((mods, base)) = control_key(c) {
            let mods = Mods {
                ctrl: mods.ctrl || extra.ctrl,
                alt: extra.alt,
                shift: extra.shift,
                super_key: extra.super_key,
            };
            self.push_special(mods.label(&base));
        }
    }

    /// Parse what follows an `ESC` that the parser has read.
    fn escape(&mut self) {
        let Some(next) = self.peek() else {
            self.push_special(ESCAPE_LABEL.to_string());
            return;
        };
        if next == ESC {
            self.push_special(ESCAPE_LABEL.to_string());
            return;
        }
        self.pos += 1;
        let at_end = self.peek().is_none();
        match next {
            '[' if !at_end => self.csi(),
            'O' if !at_end => self.ss3(),
            ']' if self.skip_string(true) => {}
            'P' | '_' if self.skip_string(false) => {}
            c if c.is_control() => self.push_control(c, Mods::NONE.with_alt()),
            ' ' => self.push_special(Mods::NONE.with_alt().label(SPACE)),
            c => self.push_special(modified_label(Mods::NONE.with_alt(), c)),
        }
    }

    /// Skip an OSC, DCS, or APC string up to its terminator, `ESC \`, or BEL
    /// when `bel_ends` is true. Answer false and move nothing when the data
    /// holds no terminator: then the start of the string is an Alt key.
    fn skip_string(&mut self, bel_ends: bool) -> bool {
        let rest = self.chars.get(self.pos..).unwrap_or_default();
        let end = rest.iter().enumerate().find_map(|(index, &c)| {
            if bel_ends && c == BEL {
                Some(index + 1)
            } else if c == ESC && rest.get(index + 1) == Some(&STRING_TERMINATOR) {
                Some(index + 2)
            } else {
                None
            }
        });
        end.inspect(|length| self.pos += length).is_some()
    }

    /// Parse an SS3 sequence. The parser has read `ESC O`.
    fn ss3(&mut self) {
        let Some(c) = self.next_char() else {
            return;
        };
        match ss3_key(c) {
            Some(Ss3Key::Special(label)) => self.push_special(label.to_string()),
            Some(Ss3Key::Text(text)) => self.push_text(text.encode_utf8(&mut [0; 4])),
            None => {}
        }
    }

    /// Read characters while `keep` holds, and give them as a string.
    fn take_while(&mut self, keep: impl Fn(char) -> bool) -> String {
        let mut taken = String::new();
        while let Some(c) = self.peek().filter(|c| keep(*c)) {
            taken.push(c);
            self.pos += 1;
        }
        taken
    }

    /// Parse a CSI sequence. The parser has read `ESC [`.
    fn csi(&mut self) {
        let params = self.take_while(|c| ('\u{30}'..='\u{3f}').contains(&c));
        let intermediates = self.take_while(|c| ('\u{20}'..='\u{2f}').contains(&c));
        let Some(final_char) = self.peek().filter(|c| ('\u{40}'..='\u{7e}').contains(c)) else {
            // The data ends, or the next character is not part of a sequence.
            // The characters read so far give no key.
            return;
        };
        self.pos += 1;
        if final_char == 'M' && params.is_empty() && intermediates.is_empty() {
            // An X10 mouse report: three more characters.
            self.pos = self.chars.len().min(self.pos + 3);
            return;
        }
        if params.starts_with(['<', '=', '>', '?']) {
            return;
        }
        let params = CsiParams::new(&params);
        if params.event_type() == EVENT_RELEASE {
            return;
        }
        match final_char {
            '~' => self.csi_tilde(&params),
            'u' => self.csi_u(&params),
            c => {
                if let Some(base) = csi_letter_label(c) {
                    let mods = if c == 'Z' {
                        params.mods().with_shift()
                    } else {
                        params.mods()
                    };
                    self.push_special(mods.label(base));
                }
            }
        }
    }

    /// Parse a CSI key with `~` as final character.
    fn csi_tilde(&mut self, params: &CsiParams) {
        if params.code() == PASTE_START_CODE {
            self.skip_paste();
            self.push_special(PASTE.to_string());
        } else if let Some(code) = params
            .other_key_code()
            .filter(|_| params.code() == MODIFY_OTHER_KEYS_CODE)
        {
            self.code_key(code, params.mods());
        } else if let Some(base) = csi_tilde_label(params.code()) {
            self.push_special(params.mods().label(base));
        }
    }

    /// Skip the text of a bracketed paste, and its end marker when it is there.
    fn skip_paste(&mut self) {
        let rest = self.chars.get(self.pos..).unwrap_or_default();
        let end = rest
            .windows(PASTE_END.len())
            .position(|window| window == PASTE_END);
        self.pos = match end {
            Some(offset) => self.pos + offset + PASTE_END.len(),
            None => self.chars.len(),
        };
    }

    /// Parse a key of the kitty keyboard protocol, with `u` as final character.
    fn csi_u(&mut self, params: &CsiParams) {
        self.code_key(params.code(), params.mods());
    }

    /// Add the key with the Unicode code `code` and the modifiers `mods`, as
    /// the kitty protocol and the xterm modifyOtherKeys form give it.
    fn code_key(&mut self, code: u32, mods: Mods) {
        let named = match code {
            13 => Some(ENTER),
            9 => Some(TAB),
            27 => Some(ESCAPE_LABEL),
            127 | 8 => Some(BACKSPACE),
            32 if !mods.is_none() => Some(SPACE),
            _ => None,
        };
        if let Some(base) = named {
            self.push_special(mods.label(base));
            return;
        }
        if PRIVATE_USE_CODES.contains(&code) {
            return;
        }
        let Some(c) = char::from_u32(code).filter(|c| !c.is_control()) else {
            return;
        };
        if mods.is_none() {
            self.push_text(c.encode_utf8(&mut [0; 4]));
        } else {
            self.push_special(modified_label(mods, c));
        }
    }
}

#[cfg(test)]
mod tests;
