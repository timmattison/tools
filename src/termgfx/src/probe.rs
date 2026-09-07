//! The question this crate asks the terminal, and what it makes of the answer.
//!
//! The rest of this crate reads the environment and names the terminal from
//! what it finds. That answer is a guess, and it is wrong in the one place a
//! guess costs the most: a terminal that carries no name of its own. A pane of
//! a multiplexer and a session of mosh both arrive that way, and both of them
//! draw pictures.
//!
//! A terminal answers two questions about the sequences it reads. The kitty
//! graphics protocol carries a query action, and a terminal that draws it
//! answers `OK`. The primary device attributes carry parameter 4, which names
//! sixel. So the sentence this crate was written on, "a terminal answers no
//! question about the ones it reads", is false for two of the three protocols.
//!
//! # Why the answer is worth a round trip
//!
//! mosh is the case that pays for it. A mosh session runs the emulator on the
//! server, and that emulator answers both questions for the **pair** — mosh
//! together with the terminal of the user. So a program inside a mosh session
//! learns what the far terminal draws, and it learns it from the one party
//! that knows. No environment variable carries that answer, because no
//! variable crosses the session.
//!
//! # The shape of the round trip
//!
//! [`ask_the_terminal`] writes [`IMAGE_QUERY`] to the controlling terminal and
//! reads until the answer of the primary device attributes arrives or until
//! the budget is spent. **The attributes request stands last, and it must stay
//! last.** Every terminal answers it, so its answer is what ends the read. A
//! terminal that draws no kitty graphics answers the first question with
//! silence, and a read that waited for that silence would spend the whole
//! budget on every run.
//!
//! # The third question, which is how big one character cell is
//!
//! A terminal lays text out in cells and it draws a picture in pixels, so a
//! tool that wants a picture of a given number of cells has to convert. The
//! `TIOCGWINSZ` ioctl carries that measure, and a mosh session carries none:
//! the mosh wire protocol resizes with a width and a height in cells and
//! nothing else, so the server writes a zero into both pixel fields of the
//! pseudo terminal. A pane of Zellij and a ttyd panel report none either.
//!
//! The xterm window operations carry the answer. [`CELL_SIZE_REQUEST`] names
//! one cell directly, and [`TEXT_AREA_REQUEST`] names the whole text area,
//! which measures a cell after a division by the cell counts of that same
//! window. [`read_cell`] puts the two in order.
//!
//! **Both questions ride in [`IMAGE_QUERY`], in front of the attributes
//! request.** A terminal answers in the order it reads, so their answers stand
//! in front of the answer that ends the read, and the two of them cost no
//! extra round trip and no extra wait. GitHub issue #468 reports what the
//! estimate of a cell did to a mosh session: `ic` drew a picture about 7
//! percent too narrow.
//!
//! # The second question, which the picture itself asks
//!
//! A kitty terminal also answers a picture that it refused, and it names a
//! code such as `ENOSPC` in the place where `OK` stands. mosh is the case that
//! pays for this question as well: the image store of a mosh session holds a
//! fixed number of bytes, and it refuses a transmission above that number. A
//! tool that reads no refusal writes the picture, ends with no error, and
//! leaves the user in front of an empty screen.
//!
//! That question needs no query of its own, because the picture is the
//! question. So [`ask_for_a_refusal`] writes [`ATTRIBUTES_REQUEST`] alone. A
//! terminal answers in the order it reads, so the refusal of the picture that
//! went before stands in front of the answer that ends the read, and a picture
//! that drew costs no wait at all.
//!
//! # Which picture a refusal is about
//!
//! The read takes every byte that stands in front of that answer, and the
//! refusal of the picture that went before is not the only thing there. The
//! answer of the run before this one arrives late, and a second program that
//! draws kitty pictures on the same terminal writes an answer of its own. A
//! reader that took the first refusal it saw would report a failure for a
//! picture that drew, and a false failure over a good picture is worse than
//! the silence that issue #465 reports.
//!
//! Every still picture carries an image number of its own, and the terminal
//! writes that number in the control keys of the answer. So
//! [`ask_for_a_refusal`] takes the number of the picture that went before, and
//! [`read_refusal`] walks past every refusal that names another one.
//!
//! # The run that owns no terminal
//!
//! The question needs raw mode, and the call that asks for raw mode sends
//! SIGTTOU to a caller that stands in a background process group. The default
//! action of SIGTTOU stops the process. So the probe asks
//! [`owns_the_terminal`] first, and a run that owns the terminal is the only
//! run that asks it anything.
//!
//! # The answer that arrives behind the budget
//!
//! The budget ends the read, and it ends no answer of a terminal. A terminal
//! that answers late writes those bytes to the descriptor the shell of the
//! user reads next, and the shell takes them for keystrokes. So the budget is
//! generous enough that a terminal on the far side of a network reaches it.
//!
//! The two questions about a cell add no shape to that risk. A terminal
//! answers in the order it reads, and both of them stand in front of the
//! request that ends the read, so a terminal that answers at all writes their
//! answers before the one the read waits for. A terminal that answers neither
//! writes nothing for them, and the read still ends on the attributes.
//!
//! **What the two questions do change is how many runs ask anything.** The
//! probe used to run for a terminal that carried no name at all. It now runs
//! for every window that reports no pixel size, a named terminal included, so
//! a mosh session and a pane of Zellij reach it. Those runs carry the risk
//! that a run of an unnamed terminal always carried, and they carry it for the
//! same budget. The trade is a picture of the right size against a terminal
//! that answers nothing writing nothing late, and issue #468 measured what the
//! guess costs: a picture about 7 percent too narrow.

use crate::detect::AnsweredProtocol;
use crate::draw::{ImageNumber, KITTY_IMAGE_NUMBER_KEY};
use crate::geometry::CellPixels;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::io::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

/// The query action of the kitty graphics protocol.
///
/// It is a transmission of one pixel that the terminal answers and never
/// draws. A terminal that reads the protocol answers `OK`, and one that reads
/// none of it answers nothing at all.
const KITTY_QUERY: &[u8] = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

/// The questions [`ask_the_terminal`] writes, in the order it writes them.
///
/// They leave in one write, so the whole set costs one round trip. A terminal
/// answers in the order it reads, so each answer arrives in the order of this
/// list.
///
/// **The attributes request stands last, and it must stay last.** Every
/// terminal answers it, so its answer is what ends the read. A question in
/// front of it costs no extra round trip and no extra wait, because its answer
/// arrives in front of the one the read waits for. A question behind it would
/// answer after the read had already stopped, and those bytes would land on
/// the descriptor the shell of the user reads next. The order is a property of
/// this list, and `the_query_asks_every_question_and_ends_with_the_attributes_request`
/// holds it.
pub(crate) const IMAGE_QUERY: [&[u8]; 4] = [
    KITTY_QUERY,
    CELL_SIZE_REQUEST,
    TEXT_AREA_REQUEST,
    ATTRIBUTES_REQUEST,
];

/// The request of the primary device attributes.
///
/// Every terminal answers this request, so its answer is what ends a read.
/// [`IMAGE_QUERY`] ends with it, and [`ask_for_a_refusal`] writes it alone.
const ATTRIBUTES_REQUEST: &[u8] = b"\x1b[c";

/// The request of the size of one character cell in pixels.
///
/// This is window operation 16 of xterm, and a terminal answers it with
/// `CSI 6 ; height ; width t`. It names one cell directly, so it is the best
/// answer a terminal gives to the question this crate asks about a cell.
const CELL_SIZE_REQUEST: &[u8] = b"\x1b[16t";

/// The first parameter of the answer to [`CELL_SIZE_REQUEST`].
const CELL_SIZE_ANSWER: &[u8] = b"6";

/// The request of the size of the text area in pixels.
///
/// This is window operation 14 of xterm, and a terminal answers it with
/// `CSI 4 ; height ; width t`. It names the whole text area and not one cell,
/// so a reader of it divides by the cell counts of that same window. It is the
/// fallback of [`CELL_SIZE_REQUEST`], because a terminal that answers the
/// older of the two operations answers this one.
const TEXT_AREA_REQUEST: &[u8] = b"\x1b[14t";

/// The first parameter of the answer to [`TEXT_AREA_REQUEST`].
const TEXT_AREA_ANSWER: &[u8] = b"4";

/// How long a reader of the terminal waits for the answer.
///
/// Two readers take this budget. [`ask_the_terminal`] waits this long for the
/// answer to a protocol query. [`ask_for_a_refusal`] waits this long for the
/// answer that says whether the terminal refused the picture that went before.
pub(crate) const QUERY_BUDGET: Duration = Duration::from_millis(500);

/// The protocol the answer of a terminal names, or [`None`] for an answer that
/// names neither.
///
/// A kitty answer outranks a sixel one. A terminal that draws both draws a
/// kitty transmission with no palette and no band, so the picture keeps every
/// colour it arrived with.
pub(crate) fn read_answer(answer: &[u8]) -> Option<AnsweredProtocol> {
    if kitty_said_ok(answer) {
        Some(AnsweredProtocol::Kitty)
    } else if attributes_name_sixel(answer) {
        Some(AnsweredProtocol::Sixel)
    } else {
        None
    }
}

/// The cell that the terminal named, from the best answer it gave.
///
/// A terminal gives two answers about a cell, and they can both arrive in one
/// buffer. [`CELL_SIZE_REQUEST`] names one cell directly, and
/// [`TEXT_AREA_REQUEST`] names the whole text area, which measures a cell only
/// after a division that rounds. So the first one outranks the second, and
/// this function is the one place that says so.
///
/// # Arguments
/// * `answer` - Every byte the terminal wrote before the answer that ended the
///   read.
/// * `cells` - The columns and the rows of the window the answers are about,
///   or `None` for a run that measured no window.
///
/// # Returns
/// The cell the terminal named, or `None` when it named none. A terminal that
/// reads no window operation answers neither question, and it reaches this
/// function as silence.
pub(crate) fn read_cell(answer: &[u8], cells: Option<(u32, u32)>) -> Option<CellPixels> {
    read_cell_size(answer).or_else(|| read_text_area_cell(answer, cells))
}

/// The size of one character cell that a terminal named in its answer to
/// [`CELL_SIZE_REQUEST`].
///
/// The answer is `CSI 6 ; height ; width t`. **The height stands first**, and
/// nothing in the bytes says so, which is why [`CellPixels::measured`] takes
/// the two numbers by name.
///
/// # Arguments
/// * `answer` - Every byte the terminal wrote before the answer that ended the
///   read.
///
/// # Returns
/// The cell that the terminal named, or `None` for an answer that names none.
fn read_cell_size(answer: &[u8]) -> Option<CellPixels> {
    let parameters = window_operation_parameters(answer, CELL_SIZE_ANSWER)?;
    // The answer carries three parameters and no other count is this answer.
    // A request to resize a window carries three of its own behind another
    // first parameter, and the guard above already put that one aside.
    let [_, height, width] = parameters.as_slice() else {
        return None;
    };
    CellPixels::measured(number(width)?, number(height)?)
}

/// The size of one character cell that the answer to [`TEXT_AREA_REQUEST`]
/// measures.
///
/// The answer names the whole text area, so one cell is the pixel width over
/// the column count and the pixel height over the row count. The division uses
/// the cell counts of the **same** window that the answer is about, which the
/// caller measured in the one read it made before it asked anything.
///
/// # Arguments
/// * `answer` - Every byte the terminal wrote before the answer that ended the
///   read.
/// * `cells` - The columns and the rows of that same window, or `None` for a
///   run that measured no window.
///
/// # Returns
/// The cell that the division measures, or `None` for an answer that names no
/// text area, for a run that measured no window to divide by, and for a
/// quotient that is no cell.
fn read_text_area_cell(answer: &[u8], cells: Option<(u32, u32)>) -> Option<CellPixels> {
    let (columns, rows) = cells?;
    let parameters = window_operation_parameters(answer, TEXT_AREA_ANSWER)?;
    let [_, height, width] = parameters.as_slice() else {
        return None;
    };
    // `Window::measured` makes no window of zero columns and no window of zero
    // rows, so no caller of a measured window reaches a division by zero here.
    // The signature takes a bare pair all the same, so the division is a
    // checked one and a zero gives no cell instead of a panic.
    // `CellPixels::measured` then refuses a quotient of no pixels, which is
    // what a text area smaller than its own grid gives.
    CellPixels::measured(
        number(width)?.checked_div(columns)?,
        number(height)?.checked_div(rows)?,
    )
}

/// The opener of a control sequence.
///
/// This is not [`ATTRIBUTES_OPENER`], which carries the `?` that opens a
/// private answer. A window operation answers with no private byte, so a
/// reader of one starts here.
const CSI_OPENER: &[u8] = b"\x1b[";

/// The final byte of every window operation and of every answer to one.
const WINDOW_OPERATION_FINAL: u8 = b't';

/// The parameters of the first window operation of `answer` whose first
/// parameter is `kind`.
///
/// The buffer holds every byte the terminal wrote before the answer that ended
/// the read, so it holds the answers of the other questions of
/// [`IMAGE_QUERY`] as well. This walk therefore passes over every sequence
/// that is not the one asked for, instead of reading the first sequence it
/// finds.
///
/// # Arguments
/// * `answer` - Every byte the terminal wrote.
/// * `kind` - The first parameter that names the answer, such as
///   [`CELL_SIZE_ANSWER`].
///
/// # Returns
/// Every parameter of that answer, the first one included, or `None` when the
/// buffer holds no such answer and when a sequence is cut short before its
/// final byte.
fn window_operation_parameters<'a>(answer: &'a [u8], kind: &[u8]) -> Option<Vec<&'a [u8]>> {
    let mut rest = answer;
    while let Some(start) = position_of(rest, CSI_OPENER) {
        let body = &rest[start + CSI_OPENER.len()..];
        // A control sequence ends at its final byte, which stands above every
        // parameter byte. A sequence with no final byte was cut short, and the
        // walk below reads the rest of the buffer all the same.
        if let Some(end) = body.iter().position(|byte| (0x40..=0x7e).contains(byte)) {
            if body[end] == WINDOW_OPERATION_FINAL {
                let parameters: Vec<&[u8]> = body[..end]
                    .split(|byte| *byte == PARAMETER_SEPARATOR)
                    .collect();
                if parameters.first() == Some(&kind) {
                    return Some(parameters);
                }
            }
        }
        // The walk goes on from the body of the sequence it just refused, and
        // not from behind the final byte of it. A sequence that was cut short
        // holds the opener of the next one inside what would otherwise be its
        // parameters, and that opener stands in front of the final byte. The
        // body is two bytes shorter than `rest` on every turn, so the walk ends.
        rest = body;
    }
    None
}

/// The number that `bytes` spells, for a run of ASCII digits and nothing else.
///
/// A parameter of a control sequence is a run of digits. Every other shape is
/// no number of this protocol, and `None` is the answer for it. That includes
/// an empty parameter, which a terminal writes for a value it left out, and a
/// number above the range, which no font has.
fn number(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

/// The opener of an application-program command, which carries a kitty answer.
const APC_OPENER: &[u8] = b"\x1b_";

/// The string terminator that ends one.
const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// The opener of an answer of the primary device attributes.
const ATTRIBUTES_OPENER: &[u8] = b"\x1b[?";

/// The final byte of an answer of the primary device attributes.
const ATTRIBUTES_FINAL: u8 = b'c';

/// The byte that divides one parameter of that answer from the next.
///
/// This byte is the byte of [`KITTY_SEPARATOR`], and that is a coincidence of
/// two protocols. The two constants stand apart for that reason: a change to
/// the answer of the attributes moves this one alone, and a change to the
/// kitty answer moves that one alone.
const PARAMETER_SEPARATOR: u8 = b';';

/// The parameter of that answer which names sixel.
const SIXEL_PARAMETER: &[u8] = b"4";

/// The message a terminal writes for a query it carried out.
const KITTY_OK: &[u8] = b"OK";

/// The key that opens the control block of a graphics answer.
const KITTY_GRAPHICS: u8 = b'G';

/// The byte that divides the control block of a kitty answer from its message.
const KITTY_SEPARATOR: u8 = b';';

/// The byte that divides one control key of a kitty answer from the next.
const KEY_SEPARATOR: u8 = b',';

/// The byte that divides the name of a control key from its value.
const KEY_VALUE_SEPARATOR: u8 = b'=';

/// One graphics answer of a kitty terminal.
///
/// The two halves arrive together, because a reader that knows what a terminal
/// said also has to know which picture the terminal said it about.
struct KittyAnswer<'a> {
    /// The control keys, without the `G` that opens the block.
    ///
    /// They are `<name>=<value>` pairs that a comma divides, such as
    /// `i=31,I=12`.
    keys: &'a [u8],
    /// What the terminal said about the command: `OK` for a command it carried
    /// out, and a code such as `ENOSPC` for one it refused.
    message: &'a [u8],
}

impl KittyAnswer<'_> {
    /// The image number that the control keys name, or [`None`] for keys that
    /// name none.
    ///
    /// The name is a capital `I`, which the crate states one time in
    /// [`KITTY_IMAGE_NUMBER_KEY`]. A lower case `i` is an image id, which names
    /// a different thing: the terminal picks an id itself for a picture that
    /// named none, so an id says nothing about which picture of this process
    /// the terminal answers.
    ///
    /// The whole value is read, so keys of `I=12` give 12 and never 1.
    fn image_number(&self) -> Option<ImageNumber> {
        let value = self
            .keys
            .split(|byte| *byte == KEY_SEPARATOR)
            .find_map(|pair| {
                let divide = pair.iter().position(|byte| *byte == KEY_VALUE_SEPARATOR)?;
                let (name, value) = pair.split_at(divide);
                // The value opens with the byte that divided it from the name.
                (name == KITTY_IMAGE_NUMBER_KEY.as_bytes()).then_some(&value[1..])
            })?;
        ImageNumber::new(std::str::from_utf8(value).ok()?.parse().ok()?)
    }

    /// Whether this answer could speak about the picture that carried `number`.
    ///
    /// An answer that names another image number belongs to another picture:
    /// to the picture of the run before this one, which the terminal answered
    /// late, or to the picture of a second program that draws on the same
    /// terminal. This call refuses that one.
    ///
    /// **An answer that names no image number at all is taken.** A strict rule
    /// would refuse it, and a terminal that reports a failure without the
    /// number of the picture would then go silent. That silence is the defect
    /// of issue #465, so the strict rule takes a report away that this crate
    /// makes today. The rule here takes none away, and it still takes the
    /// misattribution away.
    ///
    /// # Arguments
    /// * `number` - The image number that the picture of this process carried.
    ///
    /// # Returns
    /// True for an answer of that number and for an answer of no number.
    fn could_be_for(&self, number: ImageNumber) -> bool {
        self.image_number().is_none_or(|named| named == number)
    }
}

/// Every kitty graphics answer that `answer` carries, in the order the terminal
/// wrote them.
///
/// One answer is `ESC _ G <control keys> ; <message> ESC \`, and the walk gives
/// both halves of it. [`read_refusal`] reads the control keys, because the
/// image number in them says which picture the terminal is speaking about.
///
/// **This is the one walk over those blocks.** [`kitty_said_ok`] reads it to
/// learn that a query drew, and [`read_refusal`] reads it to learn that a
/// picture did not. A second walk would part company with this one the day
/// either learned something about the shape, and no test would say so.
///
/// The walk ends at a block that no string terminator closes, because such a
/// block is half of an answer that arrived late, and the bytes of the other
/// half say nothing yet.
fn kitty_messages(answer: &[u8]) -> impl Iterator<Item = KittyAnswer<'_>> + '_ {
    let mut rest = answer;
    std::iter::from_fn(move || {
        while let Some(start) = position_of(rest, APC_OPENER) {
            let body = &rest[start + APC_OPENER.len()..];
            let end = position_of(body, STRING_TERMINATOR)?;
            let block = &body[..end];
            rest = &body[end + STRING_TERMINATOR.len()..];
            // The `G` opens the block and belongs to neither half, so the keys
            // start behind it.
            let Some((&KITTY_GRAPHICS, keys_and_message)) = block.split_first() else {
                continue;
            };
            if let Some(separator) = keys_and_message
                .iter()
                .position(|byte| *byte == KITTY_SEPARATOR)
            {
                return Some(KittyAnswer {
                    keys: &keys_and_message[..separator],
                    message: &keys_and_message[separator + 1..],
                });
            }
        }
        None
    })
}

/// Whether the terminal carried out the query of [`IMAGE_QUERY`].
///
/// The message of the answer is `OK` for a query the terminal carried out. A
/// terminal that refused it writes a code such as `ENOTSUPP` in the same
/// place, which is no answer of yes.
fn kitty_said_ok(answer: &[u8]) -> bool {
    kitty_messages(answer).any(|block| block.message.starts_with(KITTY_OK))
}

/// A kitty graphics command that the terminal refused, in the words of the
/// terminal.
///
/// A refusal carries a code such as `ENOSPC`, and it carries a detailed
/// message behind that code when the terminal wrote one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The code the terminal named, such as `ENOSPC` or `ETOODEEP`.
    pub code: String,
    /// What the terminal said behind that code, or an empty string for a code
    /// that arrived alone.
    pub message: String,
}

impl std::fmt::Display for Refusal {
    /// Write the code, and the detailed message behind it when the terminal
    /// wrote one.
    ///
    /// A colon and a space divide the two, because a user reads this line and
    /// the code alone sends that user to a search engine.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.message.is_empty() {
            write!(out, "{}", self.code)
        } else {
            write!(out, "{}: {}", self.code, self.message)
        }
    }
}

/// The byte that divides the code of a refusal from its detailed message.
const REFUSAL_SEPARATOR: u8 = b':';

/// The first refusal in `answer` that could be about the picture of `number`,
/// or [`None`] for an answer that carries none.
///
/// The walk gives the first one, and it walks every block to find it. A picture
/// travels in more than one command, so the block that reports the refusal
/// stands behind blocks that report success.
///
/// A block that names another image number is walked past, and
/// [`KittyAnswer::could_be_for`] states which blocks those are. Such a block
/// holds the refusal of another picture, and a report of it would name a
/// picture that drew.
///
/// # Arguments
/// * `answer` - The bytes that the terminal wrote.
/// * `number` - The image number that the picture of this process carried.
///
/// # Returns
/// The refusal of that picture, for an answer that carries one.
fn read_refusal(answer: &[u8], number: ImageNumber) -> Option<Refusal> {
    kitty_messages(answer)
        .filter(|block| block.could_be_for(number))
        .find_map(|block| refusal_in(block.message))
}

/// The refusal that one message carries, or [`None`] for a message that
/// reports no failure.
///
/// The message is `OK` for a command the terminal carried out, and a code such
/// as `ENOSPC` for one it refused. The code stands alone or a colon divides it
/// from a detailed message, so a message with no colon in it is a whole
/// refusal and the detail of it is empty.
fn refusal_in(message: &[u8]) -> Option<Refusal> {
    let divide = message
        .iter()
        .position(|byte| *byte == REFUSAL_SEPARATOR)
        .unwrap_or(message.len());
    let code = &message[..divide];
    if code.is_empty() || code == KITTY_OK {
        return None;
    }
    Some(Refusal {
        code: text_of(code),
        message: text_of(message.get(divide + 1..).unwrap_or_default()),
    })
}

/// The text of `bytes`, with every byte that stands for no character replaced.
///
/// The specification gives a message printable ASCII characters and spaces
/// alone. A terminal that writes something else writes it into a line that a
/// user reads, and a lossy read puts that line in front of the user where a
/// strict one would take the refusal away and report nothing at all.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether an answer of the primary device attributes names sixel.
///
/// The answer is `ESC [ ? <parameters> c`, and the parameters are numbers
/// divided by a semicolon. Parameter 4 names sixel. The test is over the whole
/// parameter and not over the digit, because 14 and 41 hold a four and name
/// something else.
fn attributes_name_sixel(answer: &[u8]) -> bool {
    attributes_parameters(answer).is_some_and(|parameters| {
        parameters
            .split(|byte| *byte == PARAMETER_SEPARATOR)
            .any(|parameter| parameter == SIXEL_PARAMETER)
    })
}

/// The parameters of the first whole answer of the primary device attributes.
///
/// [`drain`] reads this to learn that the answer arrived, and
/// [`attributes_name_sixel`] reads it to learn what the answer says. One
/// reader of the shape keeps the two from disagreeing about where an answer
/// ends.
fn attributes_parameters(answer: &[u8]) -> Option<&[u8]> {
    let mut rest = answer;
    while let Some(start) = position_of(rest, ATTRIBUTES_OPENER) {
        let body = &rest[start + ATTRIBUTES_OPENER.len()..];
        // A control sequence ends at its final byte, which stands above every
        // parameter byte. An answer with no final byte is an answer cut short.
        let end = body.iter().position(|byte| (0x40..=0x7e).contains(byte))?;
        if body[end] == ATTRIBUTES_FINAL {
            return Some(&body[..end]);
        }
        rest = &body[end + 1..];
    }
    None
}

/// Where `needle` starts in `haystack`.
fn position_of(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// What one terminal said for the whole of [`IMAGE_QUERY`].
///
/// The two answers travel together because one write asked for both, and a
/// caller that took them one at a time would read the terminal twice for one
/// picture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TerminalAnswer {
    /// The image protocol the terminal named, or `None` when it named none.
    pub(crate) protocol: Option<AnsweredProtocol>,
    /// The character cell the terminal named, or `None` when it named none.
    pub(crate) cell: Option<CellPixels>,
}

/// Ask the controlling terminal which image protocol it draws and how big one
/// character cell is.
///
/// # Arguments
/// * `budget` - The longest that the read waits.
/// * `cells` - The columns and the rows of the window that the caller
///   measured, or `None` when it measured none. The answer of
///   [`TEXT_AREA_REQUEST`] divides by this pair, so the measure and the answer
///   are about one window.
///
/// # Returns
/// What the terminal said. Every field is `None` when there is no controlling
/// terminal, when this run stands in a background process group and therefore
/// owns no terminal to ask (see [`owns_the_terminal`]), when the terminal
/// answers nothing inside `budget`, and when the answer names neither thing.
pub(crate) fn ask_the_terminal(budget: Duration, cells: Option<(u32, u32)>) -> TerminalAnswer {
    let Some(answer) = query_the_terminal(budget) else {
        return TerminalAnswer::default();
    };
    TerminalAnswer {
        protocol: read_answer(&answer),
        cell: read_cell(&answer, cells),
    }
}

/// Write [`IMAGE_QUERY`] to the controlling terminal and read what comes back.
///
/// The read of the terminal stands apart from the reading of the bytes, so
/// every parser above is a function of its input alone.
///
/// # Returns
/// Every byte the terminal wrote before the answer that ended the read, or
/// `None` when this run has no terminal to ask.
fn query_the_terminal(budget: Duration) -> Option<Vec<u8>> {
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open(CONTROLLING_TERMINAL)
        .ok()?;
    let fd = terminal.as_raw_fd();
    let _raw = RawMode::of(fd)?;
    // The questions leave in one write. A terminal reads them in the order of
    // the list, and its answers come back in that order.
    (&terminal).write_all(&IMAGE_QUERY.concat()).ok()?;
    (&terminal).flush().ok()?;
    Some(drain(fd, budget))
}

/// Ask the controlling terminal whether it refused the picture that went
/// before this call.
///
/// The picture is the question, so this call writes [`ATTRIBUTES_REQUEST`] and
/// no picture of its own. Every terminal answers that request, and a terminal
/// answers in the order it reads, so a refusal stands in front of the answer
/// that ends the read. A terminal that refused nothing answers the request
/// alone, and the read ends there instead of spending the whole budget on
/// silence.
///
/// The answer of the terminal also carries every answer that arrived late, so
/// `number` says which picture this call asks about. A refusal that names
/// another image number belongs to another picture, and [`read_refusal`] walks
/// past it.
///
/// # Arguments
/// * `budget` - The longest that the read waits.
/// * `number` - The image number that the picture of this process carried.
///
/// # Returns
/// Gives [`None`] when there is no controlling terminal, when this run stands
/// in a background process group and therefore owns no terminal to ask (see
/// [`owns_the_terminal`]), when the terminal answers nothing inside `budget`,
/// when the answer reports no failure, and when every failure it reports names
/// another picture.
pub(crate) fn ask_for_a_refusal(budget: Duration, number: ImageNumber) -> Option<Refusal> {
    let terminal = OpenOptions::new()
        .read(true)
        .write(true)
        .open(CONTROLLING_TERMINAL)
        .ok()?;
    let fd = terminal.as_raw_fd();
    let _raw = RawMode::of(fd)?;
    (&terminal).write_all(ATTRIBUTES_REQUEST).ok()?;
    (&terminal).flush().ok()?;
    read_refusal(&drain(fd, budget), number)
}

/// The terminal a program asks, whatever its standard output was pointed at.
const CONTROLLING_TERMINAL: &str = "/dev/tty";

/// The largest answer this module keeps.
///
/// A refusal is the larger of the two answers this module reads. The terminal
/// writes a detailed message of its own behind the refusal code, and the
/// terminal alone decides the length of that message. Every answer of
/// [`IMAGE_QUERY`] is far below the cap.
///
/// The cap is here so that a terminal that does not stop cannot grow the
/// buffer. It truncates such an answer, and it fails nothing: a refusal longer
/// than the cap arrives cut short, because [`drain`] stops at the cap, and
/// [`read_refusal`] then reads a message with no tail.
const ANSWER_LIMIT: usize = 1024;

/// How much of an answer one read takes.
///
/// One byte, because the loop tests for a whole answer of the attributes after
/// each read. A larger read takes the bytes behind the answer as well, and
/// those bytes belong to whoever reads the descriptor next, so one byte is what
/// makes the rule of [`drain`] true. A whole answer is about twenty bytes, so
/// one probe pays about twenty pairs of select(2) and read(2) for it.
const READ_CHUNK: usize = 1;

/// Read the descriptor until the answer of the attributes arrives, until the
/// budget is spent, or until the cap is reached.
///
/// **The read stops at the answer of the attributes and takes no byte after
/// it.** Every byte after that one belongs to whoever types next, and a probe
/// that swallowed it would eat a keystroke of the user.
fn drain(fd: RawFd, budget: Duration) -> Vec<u8> {
    let deadline = Instant::now() + budget;
    let mut answer = Vec::new();
    while answer.len() < ANSWER_LIMIT {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        if !waits_for_a_byte(fd, left) {
            break;
        }
        let mut chunk = [0_u8; READ_CHUNK];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        let Ok(taken) = usize::try_from(taken) else {
            break;
        };
        if taken == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..taken]);
        if attributes_parameters(&answer).is_some() {
            break;
        }
    }
    answer
}

/// Wait up to `left` for one byte to arrive on `fd`.
///
/// # Why this is select(2) and not poll(2)
///
/// **poll(2) on macOS answers POLLNVAL for a terminal.** Measured 2026-09-06
/// against a tmux pane: the descriptor is a good one, the terminal has an
/// answer waiting, and poll(2) gives 1 with `revents` of `POLLNVAL`. A caller
/// that reads that as "no answer" gets silence from every terminal on the
/// platform, and silence from a terminal is indistinguishable from a terminal
/// that draws no image. select(2) answers the same question correctly there.
fn waits_for_a_byte(fd: RawFd, left: Duration) -> bool {
    // select(2) reads the descriptor as a bit in a fixed-width set, so a
    // descriptor above the width of that set cannot be asked about at all.
    let width = RawFd::try_from(libc::FD_SETSIZE).unwrap_or(RawFd::MAX);
    if fd < 0 || fd >= width {
        return false;
    }
    // SAFETY: `fd_set` is a plain bit array, and FD_ZERO fills the whole of it
    // before FD_SET writes one bit of it.
    let mut watched: libc::fd_set = unsafe { std::mem::zeroed() };
    // SAFETY: `watched` is one initialized set and `fd` is inside its width.
    unsafe {
        libc::FD_ZERO(&mut watched);
        libc::FD_SET(fd, &mut watched);
    }
    let mut budget = libc::timeval {
        tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
        tv_usec: libc::suseconds_t::try_from(left.subsec_micros()).unwrap_or(0),
    };
    // SAFETY: the set and the budget are owned here, and the two pointers that
    // name no set are null, which select(2) reads as "ask nothing of them".
    let ready = unsafe {
        libc::select(
            fd + 1,
            &mut watched,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut budget,
        )
    };
    ready > 0
}

/// Whether the process group of this run owns `fd`.
///
/// **A caller that does not own the terminal must write no terminal setting to
/// it.** tcsetattr(3) of such a caller sends SIGTTOU to the whole process group
/// of the caller, and the default action of SIGTTOU stops the process. A shell
/// that starts a script with `&` puts that script in a background process
/// group, so a probe with no guard here freezes the script on the one call that
/// asks for raw mode, and it prints nothing that says why.
///
/// A run that owns no terminal keeps the behavior it had before the probe
/// existed: it asks nothing, and the name the environment carries is the whole
/// of the answer.
///
/// A terminal that answers nothing to tcgetpgrp(3) is one this run does not own
/// either, because the answer of that call is the one thing that says it does.
fn owns_the_terminal(fd: RawFd) -> bool {
    // SAFETY: `fd` is the descriptor of a file this module holds open.
    let foreground = unsafe { libc::tcgetpgrp(fd) };
    // SAFETY: getpgrp takes no argument, reads no memory and fails for nothing.
    foreground != -1 && foreground == unsafe { libc::getpgrp() }
}

/// The terminal held in raw mode, and put back the way it was on the way out.
///
/// A canonical terminal gives a read nothing until a newline arrives, and no
/// answer of a terminal carries one. It also echoes, which would paint the
/// answer onto the screen of the user.
struct RawMode {
    fd: RawFd,
    saved: libc::termios,
}

impl RawMode {
    /// Put `fd` in raw mode, or give [`None`] for a descriptor this process
    /// group does not own and for a descriptor that refuses.
    fn of(fd: RawFd) -> Option<Self> {
        if !owns_the_terminal(fd) {
            return None;
        }
        // SAFETY: `termios` is a plain C struct, and tcgetattr fills the whole
        // of it before anything reads it.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is the descriptor of a file this module holds open.
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        // SAFETY: `raw` is a copy of a structure tcgetattr filled.
        unsafe { libc::cfmakeraw(&mut raw) };
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        // SAFETY: same descriptor, and `raw` is fully initialized.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: the descriptor is still open, because this value lives no
        // longer than the file it was made from.
        unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.saved) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The height of the cell that the answers of these tests name, in pixels.
    ///
    /// It stands **first** in the answer of a terminal, and second in every
    /// call of [`CellPixels::measured`]. The two numbers differ, so an answer
    /// read the wrong way round fails the test instead of passing it.
    const ANSWERED_CELL_HEIGHT: u32 = 30;

    /// The width of that same cell, in pixels.
    const ANSWERED_CELL_WIDTH: u32 = 14;

    #[test]
    fn the_answer_of_the_cell_size_names_the_height_first() {
        // Kitty 0.42 answers this shape, and so does xterm. The height stands
        // in the first parameter and the width in the second, which is the
        // order of every window operation of xterm.
        assert_eq!(
            read_cell_size(b"\x1b[6;30;14t"),
            CellPixels::measured(ANSWERED_CELL_WIDTH, ANSWERED_CELL_HEIGHT),
            "the answer names the height first, so a reader that swaps the two measures a cell of the wrong shape"
        );
    }

    /// The answer of a terminal that names the cell of these tests.
    const CELL_SIZE_ANSWER_BYTES: &[u8] = b"\x1b[6;30;14t";

    #[test]
    fn one_answer_of_each_shape_that_names_no_cell_gives_no_cell() {
        // One case for each syntactic form that reaches this parser and names
        // no cell. A parser that reads a number out of any of them measures a
        // cell that no terminal reported, and every picture of the run comes
        // out at that size.
        for (answer, form) in [
            (
                &b"\x1b[8;24;80t"[..],
                "a request to resize the window, which carries three parameters of its own behind another first one",
            ),
            (&b"\x1b[6;16t"[..], "one parameter short"),
            (&b"\x1b[6;16;8;4t"[..], "one parameter long"),
            (&b"\x1b[6;0;14t"[..], "a height of no pixels"),
            (&b"\x1b[6;30;0t"[..], "a width of no pixels"),
            (
                &b"\x1b[6;99999;99999t"[..],
                "a cell far above the largest font",
            ),
            (&b"\x1b[6;;14t"[..], "a height the terminal left out"),
            (&b"\x1b[6;30;14"[..], "an answer cut short of its final byte"),
            (&b""[..], "a terminal that answered nothing at all"),
        ] {
            assert_eq!(
                read_cell_size(answer),
                None,
                "an answer of {form} names no cell, and the measure must fall through to the next source"
            );
        }
    }

    #[test]
    fn a_sequence_cut_short_in_front_of_the_answer_does_not_take_the_answer_with_it() {
        // The buffer holds every byte that stood in front of the answer which
        // ended the read, and a terminal that answered the run before this one
        // late leaves a part of a sequence there. The opener of the real answer
        // stands inside what those bytes would otherwise claim as parameters.
        let mut answer = b"\x1b[999".to_vec();
        answer.extend_from_slice(CELL_SIZE_ANSWER_BYTES);

        assert_eq!(
            read_cell_size(&answer),
            CellPixels::measured(ANSWERED_CELL_WIDTH, ANSWERED_CELL_HEIGHT),
            "a walk that steps over the final byte of a sequence cut short steps over the opener of the next one with it"
        );
    }

    /// The columns and the rows of the window that the tests of the text area
    /// divide by.
    const ANSWERED_WINDOW_CELLS: (u32, u32) = (80, 24);

    #[test]
    fn the_answer_of_the_text_area_divides_by_the_cells_of_the_same_window() {
        // 640 pixels over 80 columns is a cell 8 pixels wide, and 384 pixels
        // over 24 rows is a cell 16 pixels tall. The height stands first in
        // this answer as well.
        assert_eq!(
            read_text_area_cell(b"\x1b[4;384;640t", Some(ANSWERED_WINDOW_CELLS)),
            CellPixels::measured(8, 16),
            "the answer names the whole text area, and the cell counts of that same window name one cell"
        );
        assert_eq!(
            read_text_area_cell(b"\x1b[4;384;640t", None),
            None,
            "a run that measured no window holds nothing to divide by, so it measures no cell"
        );
    }

    #[test]
    fn the_cell_that_the_cell_size_names_outranks_the_one_the_text_area_measures() {
        // The two answers arrive in one buffer, because the query asks both
        // questions in one write. They disagree here, so the order of the two
        // sources is the whole of what this test measures.
        let both = b"\x1b[6;30;14t\x1b[4;384;640t";

        assert_eq!(
            read_cell(both, Some(ANSWERED_WINDOW_CELLS)),
            CellPixels::measured(ANSWERED_CELL_WIDTH, ANSWERED_CELL_HEIGHT),
            "the answer of the cell size names one cell directly, and the text area names one only after a division that rounds"
        );
        assert_eq!(
            read_cell(b"\x1b[4;384;640t", Some(ANSWERED_WINDOW_CELLS)),
            CellPixels::measured(8, 16),
            "a terminal that answers the text area alone still measures a cell"
        );
        assert_eq!(
            read_cell(b"\x1b[?62;4c", Some(ANSWERED_WINDOW_CELLS)),
            None,
            "a terminal that reads no window operation answers neither question, and it names no cell"
        );
    }

    #[test]
    fn an_answer_that_names_sixel_gives_sixel() {
        // tmux 3.7c answers this, measured 2026-09-06. Parameter 4 names sixel.
        assert_eq!(
            read_answer(b"\x1b[?1;2;4c"),
            Some(AnsweredProtocol::Sixel),
            "parameter 4 of the primary device attributes names sixel"
        );
    }

    #[test]
    fn a_zellij_answer_names_sixel() {
        // zellij 0.45.0 answers this, measured 2026-09-06.
        assert_eq!(
            read_answer(b"\x1bP>|Zellij(4500)\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Sixel)
        );
    }

    #[test]
    fn an_ok_of_the_kitty_query_gives_kitty() {
        assert_eq!(
            read_answer(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4;52c"),
            Some(AnsweredProtocol::Kitty),
            "a terminal that answers both draws the kitty transmission"
        );
    }

    #[test]
    fn an_answer_that_names_neither_gives_nothing() {
        assert_eq!(read_answer(b"\x1b[?62;22c"), None);
    }

    #[test]
    fn a_refusal_of_the_kitty_query_is_no_kitty_answer() {
        // The specification answers a refused query with a code, not with OK.
        assert_eq!(
            read_answer(b"\x1b_Gi=31;ENOTSUPP:nope\x1b\\\x1b[?62;22c"),
            None
        );
    }

    #[test]
    fn a_parameter_that_merely_holds_a_four_is_no_sixel() {
        // 14 and 41 are not 4. A test of the digit alone would take them.
        assert_eq!(read_answer(b"\x1b[?14;41c"), None);
    }

    #[test]
    fn silence_gives_nothing() {
        assert_eq!(read_answer(b""), None);
    }

    #[test]
    fn a_descriptor_outside_the_set_of_select_waits_for_nothing() {
        // select(2) reads the descriptor as a bit of a fixed-width set, so a
        // descriptor above that width would write past the end of it.
        let width = i32::try_from(libc::FD_SETSIZE).expect("the set of select is far below i32");
        assert!(!waits_for_a_byte(width, Duration::from_millis(1)));
        assert!(!waits_for_a_byte(-1, Duration::from_millis(1)));
    }

    #[test]
    fn the_read_stops_at_the_answer_and_leaves_what_follows() {
        // A user who types inside the budget writes those bytes to the same
        // descriptor the answer of the terminal arrives on. The probe owes
        // every one of them to whoever reads the descriptor next.
        let mut ends = [0_i32; 2];
        // SAFETY: pipe(2) fills the two descriptors of an array this test owns.
        let made = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(made, 0, "pipe(2) gives a pair of descriptors");
        let [reader, writer] = ends;

        let sent = b"\x1b[?1;2;4cxyz";
        // SAFETY: the buffer is owned here and the length is its own.
        let put = unsafe { libc::write(writer, sent.as_ptr().cast(), sent.len()) };
        assert_eq!(
            put,
            isize::try_from(sent.len()).expect("the answer is far below the size of a pipe"),
            "the whole of the answer and the keystrokes reach the pipe"
        );

        let answer = drain(reader, QUERY_BUDGET);
        assert_eq!(
            read_answer(&answer),
            Some(AnsweredProtocol::Sixel),
            "the read takes the whole answer of the attributes"
        );

        // A closed write end ends the second read at once, whatever it holds.
        // SAFETY: this test opened the descriptor and nothing else holds it.
        unsafe { libc::close(writer) };
        let mut left = [0_u8; 16];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(reader, left.as_mut_ptr().cast(), left.len()) };
        // SAFETY: this test opened the descriptor and nothing else holds it.
        unsafe { libc::close(reader) };
        let taken =
            usize::try_from(taken).expect("a read of a pipe with no writer fails for nothing");
        assert_eq!(
            &left[..taken],
            b"xyz",
            "the bytes after the answer stay for the reader that comes next"
        );
    }

    #[test]
    fn the_query_asks_every_question_and_ends_with_the_attributes_request() {
        assert!(
            IMAGE_QUERY.contains(&CELL_SIZE_REQUEST),
            "the question about one cell rides in the same write, so it costs no round trip of its own"
        );
        assert!(
            IMAGE_QUERY.contains(&TEXT_AREA_REQUEST),
            "the question about the text area rides there too, for a terminal that reads the older window operation alone"
        );
        assert_eq!(
            IMAGE_QUERY.last(),
            Some(&ATTRIBUTES_REQUEST),
            "the answer of the attributes request is what ends the read, so its request stands last"
        );
        assert!(
            IMAGE_QUERY.concat().ends_with(ATTRIBUTES_REQUEST),
            "the bytes that reach the terminal end with it as well"
        );
    }

    /// The budget of the read that gets nothing.
    ///
    /// Nothing writes to the pipe of that test, so every run of it spends the
    /// whole of this budget. It is short for that reason.
    const EMPTY_BUDGET: Duration = Duration::from_millis(200);

    /// The least that read takes.
    ///
    /// A little under the budget, because select(2) takes its own budget in
    /// whole microseconds and the nanoseconds below one microsecond are lost on
    /// the way in. A read that gave up at once takes about nothing, which is far
    /// under this.
    const EMPTY_FLOOR: Duration = Duration::from_millis(150);

    /// The most that read takes.
    ///
    /// Generous, because a loaded machine wakes a sleeper late. This test
    /// measures the code and not the machine, so the ceiling is far above the
    /// budget and only a read with no end at all reaches it.
    const EMPTY_CEILING: Duration = Duration::from_secs(5);

    #[test]
    fn the_budget_ends_a_read_that_gets_nothing() {
        // A terminal that draws no image answers the kitty query with silence,
        // and a terminal on the far side of a network answers late. The budget
        // is what ends the read for both of them. A read with no end holds the
        // whole tool, because the answer of this probe stands in front of the
        // first thing the tool draws.
        let mut ends = [0_i32; 2];
        // SAFETY: pipe(2) fills the two descriptors of an array this test owns.
        let made = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(made, 0, "pipe(2) gives a pair of descriptors");
        let [reader, writer] = ends;

        // The write end stays open across the read. A closed one gives the read
        // the end of the file at once, and the budget would then end nothing.
        let started = Instant::now();
        let answer = drain(reader, EMPTY_BUDGET);
        let took = started.elapsed();

        // Both descriptors go back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened the two descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(writer);
            libc::close(reader);
        }

        assert!(
            answer.is_empty(),
            "a pipe that nobody writes to holds no answer, and the read gave {answer:?}"
        );
        assert!(
            took >= EMPTY_FLOOR,
            "the read waits for the budget of {EMPTY_BUDGET:?}, and it came back after {took:?}"
        );
        assert!(
            took < EMPTY_CEILING,
            "the budget of {EMPTY_BUDGET:?} ends the read, and it took {took:?}"
        );
    }

    /// The budget of the probe that the background grandchild runs.
    ///
    /// Nothing stands behind the pseudo-terminal to answer, so every run of
    /// this test spends the whole of this budget. It is short for that reason.
    const BACKGROUND_BUDGET: Duration = Duration::from_millis(50);

    /// How long this test waits for the child to report.
    const REPORT_BUDGET: Duration = Duration::from_secs(10);

    /// How long this test waits between two reads of the state of the child.
    const REPORT_POLL: Duration = Duration::from_millis(10);

    /// How many bytes the child writes to name the grandchild.
    const PID_BYTES: usize = std::mem::size_of::<libc::pid_t>();

    /// What the child exits with for a grandchild that stopped.
    const STOPPED: libc::c_int = 17;

    /// What the child exits with for a grandchild that came back.
    const CAME_BACK: libc::c_int = 19;

    /// What the child exits with when a call of its own failed.
    const FAILED: libc::c_int = 21;

    /// What the child exits with when the guard refused the owner itself.
    ///
    /// The verdict of the grandchild alone says nothing about a guard that
    /// answers no to every caller, and such a guard takes the probe out of
    /// service on every terminal. So the child, which owns the terminal, asks
    /// the guard about itself and reports what it said.
    const OWNER_REFUSED: libc::c_int = 23;

    /// Ask `terminal` from a process group that does not own it, and exit with
    /// the verdict.
    ///
    /// This runs in a child of a fork, and the process it forked from is a test
    /// binary that holds many threads. So the block below holds plain libc
    /// calls alone and leaves through `_exit`, which runs no destructor of the
    /// parent.
    ///
    /// The child takes `terminal` as its controlling terminal, and its process
    /// group is the foreground group of that terminal. The grandchild leaves
    /// that group, which is what puts it in the background, and asks the
    /// terminal from there. A grandchild that stops instead of coming back is
    /// the defect. The child names the grandchild on `report` first, so that a
    /// run which has to clean up after a timeout can reach it.
    ///
    /// The child also reports what the guard says about the child itself, which
    /// owns the terminal. Both directions of the guard are then in one verdict.
    fn ask_from_the_background(terminal: RawFd, report: RawFd) -> ! {
        // SAFETY: every call below reads no memory of this process, except the
        // bytes of `named` that the write names and the status word that
        // waitpid fills, and this function owns both of them. `terminal` and
        // `report` are descriptors the test opened and still holds. The fork
        // gave this process a new process id and left it in the process group
        // of the test, so it leads no process group and setsid cannot fail for
        // the one reason it has.
        unsafe {
            if libc::setsid() == -1 {
                libc::_exit(FAILED);
            }
            #[allow(
                clippy::disallowed_methods,
                reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of this child, and termsize offers no call for that"
            )]
            if libc::ioctl(terminal, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                libc::_exit(FAILED);
            }

            // setsid made this child the leader of a session and TIOCSCTTY made
            // its process group the foreground group of the terminal, so this
            // child owns the terminal and the guard must say so.
            if !owns_the_terminal(terminal) {
                libc::_exit(OWNER_REFUSED);
            }

            let grandchild = libc::fork();
            if grandchild == -1 {
                libc::_exit(FAILED);
            }
            if grandchild == 0 {
                // A process group of its own is a background group of this
                // terminal, because the foreground group is still the one of
                // the child. This is the shape of a script that a shell
                // started with `&`.
                if libc::setpgid(0, 0) == -1 {
                    libc::_exit(FAILED);
                }
                let _answer = ask_the_terminal(BACKGROUND_BUDGET, None);
                libc::_exit(0);
            }

            let named = grandchild.to_ne_bytes();
            let _sent = libc::write(report, named.as_ptr().cast(), named.len());

            let mut status: libc::c_int = 0;
            if libc::waitpid(grandchild, &mut status, libc::WUNTRACED) == -1 {
                libc::_exit(FAILED);
            }
            if libc::WIFSTOPPED(status) {
                libc::kill(grandchild, libc::SIGCONT);
                libc::kill(grandchild, libc::SIGKILL);
                libc::waitpid(grandchild, std::ptr::null_mut(), 0);
                libc::_exit(STOPPED);
            }
            libc::_exit(CAME_BACK)
        }
    }

    /// The process id the child wrote to `reader`, or [`None`] for a child that
    /// named nothing inside `budget`.
    fn read_the_pid(reader: RawFd, budget: Duration) -> Option<libc::pid_t> {
        if !waits_for_a_byte(reader, budget) {
            return None;
        }
        let mut named = [0_u8; PID_BYTES];
        // SAFETY: the buffer is owned here and the length is its own.
        let taken = unsafe { libc::read(reader, named.as_mut_ptr().cast(), named.len()) };
        (usize::try_from(taken) == Ok(named.len())).then(|| libc::pid_t::from_ne_bytes(named))
    }

    #[test]
    fn a_run_in_the_background_asks_the_terminal_nothing() {
        // tcsetattr(3) of a caller that stands in a background process group
        // sends SIGTTOU to the whole of that group, and the default action of
        // SIGTTOU stops the process. A script that a shell started with `&`
        // stands in such a group, so a probe that puts the terminal in raw
        // mode there freezes the script and prints nothing that says why. This
        // test builds that process group and asks whether the probe comes back
        // out of it.
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // SAFETY: openpty writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The three null
        // pointers ask for the default terminal modes, for no name of the
        // slave device and for the default size of the window. The probe reads
        // no window, so no size of one reaches what this test asserts.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            opened,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );

        let mut ends = [0_i32; 2];
        // SAFETY: pipe(2) fills the two descriptors of an array this test owns.
        let made = unsafe { libc::pipe(ends.as_mut_ptr()) };
        assert_eq!(made, 0, "pipe(2) gives a pair of descriptors");
        let [reader, writer] = ends;

        // SAFETY: fork(2) reads nothing of this process. The child leaves
        // through `_exit` alone, so it runs no destructor of this one.
        let child = unsafe { libc::fork() };
        assert!(child != -1, "fork(2) must give a child");
        if child == 0 {
            ask_from_the_background(slave, writer);
        }

        // SAFETY: this test opened the descriptor and the child holds a copy of
        // its own. A closed copy here ends the read below for a child that
        // names nothing.
        unsafe { libc::close(writer) };
        let grandchild = read_the_pid(reader, REPORT_BUDGET);

        let deadline = Instant::now() + REPORT_BUDGET;
        let mut status: libc::c_int = 0;
        let mut reported = 0;
        while Instant::now() < deadline {
            // SAFETY: `status` is owned here, and `child` is a child of this
            // process, so waitpid reads no other family.
            reported = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
            if reported != 0 {
                break;
            }
            std::thread::sleep(REPORT_POLL);
        }

        if reported != child {
            // Take the family down, so that a run which timed out leaves no
            // stopped process behind. The grandchild goes first: it stands in
            // a process group of its own, and a group with no living parent
            // outside it takes no job control signal at all.
            // SAFETY: kill and waitpid read no memory of this process, and both
            // process ids below name a member of this family.
            unsafe {
                if let Some(grandchild) = grandchild {
                    libc::kill(grandchild, libc::SIGCONT);
                    libc::kill(grandchild, libc::SIGKILL);
                }
                libc::kill(child, libc::SIGKILL);
                libc::waitpid(child, std::ptr::null_mut(), 0);
            }
        }

        // Every descriptor goes back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened all three descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(reader);
            libc::close(slave);
            libc::close(master);
        }

        assert_eq!(
            reported, child,
            "the child must report inside {REPORT_BUDGET:?}"
        );
        assert!(
            libc::WIFEXITED(status),
            "the child must exit, and it exited for a signal instead"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            CAME_BACK,
            "a probe that stands in a background process group must come back. \
             {STOPPED} says the grandchild stopped, which is the SIGTTOU that \
             tcsetattr(3) sends to a background caller. {OWNER_REFUSED} says \
             the guard refused the child, which owns the terminal and must be \
             free to ask it"
        );
    }

    /// A pseudo-terminal, as the master end and then the slave end.
    ///
    /// The slave end is a terminal that no user reads. A test that puts it in
    /// raw mode, or that writes a query to it, therefore reaches nothing of the
    /// terminal of whoever started the run.
    fn open_a_pseudo_terminal() -> (RawFd, RawFd) {
        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        // SAFETY: openpty writes one file descriptor to each of the first two
        // pointers, and both point at a live local variable. The three null
        // pointers ask for the default terminal modes, for no name of the slave
        // device and for the default size of the window.
        let opened = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(
            opened,
            0,
            "openpty must give a pseudo-terminal: {}",
            std::io::Error::last_os_error()
        );
        (master, slave)
    }

    /// Wait up to [`REPORT_BUDGET`] for `child`, and give the status word of a
    /// child that reported inside it.
    ///
    /// A child that reports nothing inside the budget is taken down here, so a
    /// run that timed out leaves no process behind, and the answer is [`None`].
    fn wait_for_the_child(child: libc::pid_t) -> Option<libc::c_int> {
        let deadline = Instant::now() + REPORT_BUDGET;
        let mut status: libc::c_int = 0;
        while Instant::now() < deadline {
            // SAFETY: `status` is owned here, and `child` is a child of this
            // process, so waitpid reads no other family.
            let reported = unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) };
            if reported == child {
                return Some(status);
            }
            if reported == -1 {
                return None;
            }
            std::thread::sleep(REPORT_POLL);
        }
        // SAFETY: kill and waitpid read no memory of this process, and `child`
        // names a child of it. SIGCONT sets a child that stopped running again,
        // and SIGKILL then ends it.
        unsafe {
            libc::kill(child, libc::SIGCONT);
            libc::kill(child, libc::SIGKILL);
            libc::waitpid(child, std::ptr::null_mut(), 0);
        }
        None
    }

    /// Leave this child with `verdict`.
    ///
    /// **This is for a child of a fork alone.** The process that forked is a
    /// test binary that holds many threads, so the child runs no destructor of
    /// it and takes the one door that runs none.
    fn leave(verdict: libc::c_int) -> ! {
        // SAFETY: `_exit` reads no memory of this process and ends it at once.
        unsafe { libc::_exit(verdict) }
    }

    /// Take `terminal` as the controlling terminal of this child.
    ///
    /// setsid(2) goes first. It puts the child in a session of its own, and it
    /// drops the controlling terminal the child inherited. **That drop is what
    /// keeps a test off the terminal of whoever started the run**: a child that
    /// failed to take the pseudo-terminal owns no terminal at all, so nothing
    /// it does afterwards reaches a terminal of the user.
    ///
    /// The child is the leader of that session and `TIOCSCTTY` makes the
    /// process group of the child the foreground group of the terminal. So the
    /// child owns the terminal, and [`owns_the_terminal`] says so.
    fn claim_the_terminal(terminal: RawFd) {
        // SAFETY: setsid takes no argument and reads no memory. The fork gave
        // this process a new process id and left it in the process group of the
        // test, so it leads no process group and setsid cannot fail for the one
        // reason it has. The ioctl reads no memory either, and `terminal` is a
        // descriptor the test opened and still holds.
        unsafe {
            if libc::setsid() == -1 {
                leave(FAILED);
            }
            #[allow(
                clippy::disallowed_methods,
                reason = "the ban covers the read of a window, and `TIOCSCTTY` reads none. It claims the pseudo-terminal as the controlling terminal of this child, and termsize offers no call for that"
            )]
            if libc::ioctl(terminal, libc::c_ulong::from(libc::TIOCSCTTY), 0) == -1 {
                leave(FAILED);
            }
        }
    }

    /// The terminal mode of `fd`.
    ///
    /// **This is for a child of a fork alone.** A call that fails leaves the
    /// child with [`FAILED`], because a child that read no mode has nothing to
    /// compare and nothing to report.
    fn mode_of(fd: RawFd) -> libc::termios {
        // SAFETY: `termios` is a plain C struct, and tcgetattr fills the whole
        // of it before anything reads it.
        let mut mode: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: `fd` is a descriptor this child holds open, and `mode` is one
        // owned structure that tcgetattr fills.
        if unsafe { libc::tcgetattr(fd, &mut mode) } != 0 {
            leave(FAILED);
        }
        mode
    }

    /// The bits of the local modes that the terminal driver owns.
    ///
    /// `PENDIN` stands for input that the driver has to retype, and the driver
    /// sets that bit itself when the canonical line discipline comes back on.
    /// tcsetattr(3) cannot clear it either: the driver puts its own value of
    /// that bit into every mode a caller writes. `FLUSHO` stands for output the
    /// driver threw away, and it is such a bit as well.
    ///
    /// Measured 2026-09-06 on macOS 25.6 against the pseudo-terminal of the
    /// test below: the probe puts every other bit of the local modes back, and
    /// the terminal carries `PENDIN` afterwards whatever the probe writes. So
    /// neither bit says anything about the mode the probe put back, and a
    /// comparison that read them would name a defect in every restore of a
    /// terminal that a user works in.
    const DRIVER_MODES: libc::tcflag_t = libc::PENDIN | libc::FLUSHO;

    /// Whether two terminal modes carry the same value in every field a caller
    /// writes.
    ///
    /// The fields are read one at a time. A `termios` carries padding that no
    /// call fills, so a comparison of the whole structure would read bytes that
    /// stand for nothing. The two speeds arrive through cfgetispeed(3) and
    /// cfgetospeed(3), because the field behind them carries a different name
    /// on each platform. The local modes are read without [`DRIVER_MODES`],
    /// which no caller writes.
    fn same_mode(before: &libc::termios, after: &libc::termios) -> bool {
        // SAFETY: both structures were filled by tcgetattr, and the two calls
        // read them and write neither.
        let speeds = unsafe {
            libc::cfgetispeed(before) == libc::cfgetispeed(after)
                && libc::cfgetospeed(before) == libc::cfgetospeed(after)
        };
        speeds
            && before.c_iflag == after.c_iflag
            && before.c_oflag == after.c_oflag
            && before.c_cflag == after.c_cflag
            && (before.c_lflag & !DRIVER_MODES) == (after.c_lflag & !DRIVER_MODES)
            && before.c_cc == after.c_cc
    }

    /// What the child of the restore test exits with when every field of the
    /// mode came back.
    const RESTORED: libc::c_int = 31;

    /// What it exits with when the guard refused a terminal this child owns.
    const GUARD_REFUSED: libc::c_int = 33;

    /// What it exits with for a live [`RawMode`] that left the terminal
    /// canonical or echoing.
    const NOT_RAW: libc::c_int = 35;

    /// What it exits with when a field of the mode did not come back.
    const NOT_RESTORED: libc::c_int = 37;

    /// Make a [`RawMode`] of `terminal`, drop it, and exit with what happened
    /// to the mode of that terminal.
    ///
    /// This runs in a child of a fork, and the process it forked from is a test
    /// binary that holds many threads. So it calls libc alone, it allocates
    /// nothing, and it leaves through [`leave`].
    fn report_the_restore(terminal: RawFd) -> ! {
        claim_the_terminal(terminal);
        let before = mode_of(terminal);
        {
            let Some(_raw) = RawMode::of(terminal) else {
                leave(GUARD_REFUSED);
            };
            let live = mode_of(terminal);
            // cfmakeraw(3) turns the echo off and it turns the canonical line
            // discipline off. A terminal that still carries either one gives a
            // read nothing until a newline arrives, or it paints the answer of
            // the terminal onto the screen of the user.
            if live.c_lflag & (libc::ECHO | libc::ICANON) != 0 {
                leave(NOT_RAW);
            }
        }
        let after = mode_of(terminal);
        if same_mode(&before, &after) {
            leave(RESTORED);
        }
        leave(NOT_RESTORED)
    }

    #[test]
    fn the_terminal_goes_back_to_the_mode_it_had() {
        // The probe takes the terminal of the user out of the mode that user
        // works in. A probe that leaves it there hands the shell a terminal
        // that echoes nothing and reads no line, and the user has no way to
        // read on the screen what went wrong.
        let (master, slave) = open_a_pseudo_terminal();

        // SAFETY: fork(2) reads nothing of this process. The child leaves
        // through `_exit` alone, so it runs no destructor of this one.
        let child = unsafe { libc::fork() };
        assert!(child != -1, "fork(2) must give a child");
        if child == 0 {
            report_the_restore(slave);
        }

        let status = wait_for_the_child(child);

        // Both descriptors go back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened the two descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(slave);
            libc::close(master);
        }

        let status = status.expect("the child must report inside the budget");
        assert!(
            libc::WIFEXITED(status),
            "the child must exit, and it exited for a signal instead"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            RESTORED,
            "the mode of the terminal must come back whole. {NOT_RESTORED} says \
             a field of it did not, {NOT_RAW} says the live raw mode was no raw \
             mode, {GUARD_REFUSED} says the guard refused a child that owns the \
             terminal, and {FAILED} says a call of the child failed"
        );
    }

    /// How long the probe of the round trip waits for its answer.
    ///
    /// The answer arrives from the test itself, which writes it as soon as the
    /// query reaches the master end, so a run that works spends almost none of
    /// this. It is here for a run that answers nothing.
    const ROUND_TRIP_BUDGET: Duration = Duration::from_secs(5);

    /// How much of the query one read of the master end takes.
    const QUERY_CHUNK: usize = 64;

    /// The answer this test writes back. tmux 3.7c answers this, measured
    /// 2026-09-06, and parameter 4 of it names sixel.
    const SIXEL_ANSWER: &[u8] = b"\x1b[?1;2;4c";

    /// What the child of the round trip exits with for the answer it read.
    const NAMED_SIXEL: libc::c_int = 41;

    /// What it exits with when the probe named kitty.
    const NAMED_KITTY: libc::c_int = 43;

    /// What it exits with when the probe named nothing.
    const NAMED_NOTHING: libc::c_int = 45;

    /// Ask the controlling terminal of this child, and exit with what the probe
    /// made of the answer.
    ///
    /// This runs in a child of a fork of a test binary that holds many threads,
    /// and it leaves through [`leave`].
    fn report_the_round_trip(terminal: RawFd, master: RawFd) -> ! {
        claim_the_terminal(terminal);
        // SAFETY: the fork gave this child a copy of the master end, and this
        // child reads nothing of it. The test holds the other copy, and the
        // answer arrives on that one.
        unsafe { libc::close(master) };
        leave(match ask_the_terminal(ROUND_TRIP_BUDGET, None).protocol {
            Some(AnsweredProtocol::Sixel) => NAMED_SIXEL,
            Some(AnsweredProtocol::Kitty) => NAMED_KITTY,
            None => NAMED_NOTHING,
        })
    }

    /// Read `fd` until the whole query of the probe arrives, or until `budget`
    /// is spent.
    ///
    /// The query ends with [`ATTRIBUTES_REQUEST`], so the end of it is what
    /// says the whole of it arrived.
    fn read_the_query(fd: RawFd, budget: Duration) -> Vec<u8> {
        let deadline = Instant::now() + budget;
        let mut seen = Vec::new();
        while seen.len() < ANSWER_LIMIT && !seen.ends_with(ATTRIBUTES_REQUEST) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || !waits_for_a_byte(fd, left) {
                break;
            }
            let mut chunk = [0_u8; QUERY_CHUNK];
            // SAFETY: the buffer is owned here and the length is its own.
            let taken = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
            let Ok(taken) = usize::try_from(taken) else {
                break;
            };
            if taken == 0 {
                break;
            }
            seen.extend_from_slice(&chunk[..taken]);
        }
        seen
    }

    #[test]
    fn the_probe_asks_the_terminal_and_reads_what_it_answers() {
        // The whole round trip, over a terminal that this test answers for: the
        // probe opens the controlling terminal, puts it in raw mode, writes the
        // query, and reads the answer back off the same descriptor.
        let (master, slave) = open_a_pseudo_terminal();

        // SAFETY: fork(2) reads nothing of this process. The child leaves
        // through `_exit` alone, so it runs no destructor of this one.
        let child = unsafe { libc::fork() };
        assert!(child != -1, "fork(2) must give a child");
        if child == 0 {
            report_the_round_trip(slave, master);
        }

        // The query arrives on the master end, and it arrives after the child
        // put the terminal in raw mode. So the answer this test writes back
        // reaches a terminal that echoes nothing and waits for no newline.
        let query = read_the_query(master, REPORT_BUDGET);
        // SAFETY: the buffer is owned here and the length is its own.
        let put = unsafe { libc::write(master, SIXEL_ANSWER.as_ptr().cast(), SIXEL_ANSWER.len()) };

        let status = wait_for_the_child(child);

        // Both descriptors go back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened the two descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(slave);
            libc::close(master);
        }

        assert_eq!(
            query,
            IMAGE_QUERY.concat(),
            "the probe writes the whole query to the terminal it asks"
        );
        assert_eq!(
            put,
            isize::try_from(SIXEL_ANSWER.len())
                .expect("the answer is far below the size of a read"),
            "the whole of the answer reaches the terminal"
        );

        let status = status.expect("the child must report inside the budget");
        assert!(
            libc::WIFEXITED(status),
            "the child must exit, and it exited for a signal instead"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            NAMED_SIXEL,
            "the probe must read the answer the terminal wrote. {NAMED_NOTHING} \
             says it read no answer at all, {NAMED_KITTY} says it named the \
             other protocol, and {FAILED} says a call of the child failed"
        );
    }

    /// A refusal of the image store, and the answer of the attributes behind
    /// it.
    ///
    /// This is the shape that mosh writes. The image store of a mosh session
    /// holds a fixed number of bytes, and a transmission above that number
    /// arrives as `ENOSPC`.
    const REFUSAL_ANSWER: &[u8] = b"\x1b_Gi=31;ENOSPC:the image store is full\x1b\\\x1b[?62;4c";

    /// The code that [`REFUSAL_ANSWER`] carries.
    const REFUSED_CODE: &str = "ENOSPC";

    /// The detailed message that [`REFUSAL_ANSWER`] carries behind that code.
    const REFUSED_DETAIL: &str = "the image store is full";

    /// The image number that the picture of these tests carried.
    ///
    /// A minted number belongs to the run that minted it and no test can
    /// predict one, so these tests state a number of their own and answer for
    /// a terminal that speaks about it.
    const THIS_PICTURE_NUMBER: u32 = 7;

    /// The image number of the picture that these tests drew.
    fn this_picture() -> ImageNumber {
        ImageNumber::new(THIS_PICTURE_NUMBER).expect("the number of a test picture is above zero")
    }

    #[test]
    fn a_refusal_gives_the_code_and_the_detail() {
        // The code names what went wrong and the detail says it in words. A
        // reader of the refusal prints both, because the code alone sends the
        // user to a search engine.
        assert_eq!(
            read_refusal(REFUSAL_ANSWER, this_picture()),
            Some(Refusal {
                code: REFUSED_CODE.to_owned(),
                message: REFUSED_DETAIL.to_owned(),
            }),
            "the colon divides the code of a refusal from its detail"
        );
    }

    #[test]
    fn a_refusal_with_no_detail_gives_the_code_alone() {
        // The specification makes the detail optional, so a code stands as a
        // whole refusal. A parser that waited for a colon would read this one
        // as no refusal at all, and the picture would go missing in silence.
        assert_eq!(
            read_refusal(b"\x1b_Gi=31;ETOODEEP\x1b\\\x1b[?62;4c", this_picture()),
            Some(Refusal {
                code: "ETOODEEP".to_owned(),
                message: String::new(),
            }),
            "a code with no colon behind it is a whole refusal"
        );
    }

    #[test]
    fn an_ok_answer_is_no_refusal() {
        // `OK` is what a terminal writes for a command it carried out. A
        // reader that took it for a refusal would report a failure for every
        // picture that drew.
        assert_eq!(
            read_refusal(b"\x1b_Gi=99,I=7;OK\x1b\\", this_picture()),
            None
        );
    }

    #[test]
    fn an_answer_of_the_attributes_alone_is_no_refusal() {
        // The request of the attributes is what ends the read, so its answer
        // stands in the bytes of every round trip. It is no APC block, and the
        // reader must walk past it.
        assert_eq!(read_refusal(b"\x1b[?62;4c", this_picture()), None);
    }

    #[test]
    fn silence_is_no_refusal() {
        // A terminal that answers nothing refused nothing that this crate can
        // report. A run that owns no terminal reads the same silence.
        assert_eq!(read_refusal(b"", this_picture()), None);
    }

    #[test]
    fn a_refusal_behind_an_ok_still_arrives() {
        // A terminal answers every command it reads, and a picture travels in
        // more than one command. So the block that reports the refusal stands
        // behind blocks that report success, and a reader that stopped at the
        // first block would report that the picture drew.
        assert_eq!(
            read_refusal(
                b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=31;ENOSPC\x1b\\\x1b[?62;4c",
                this_picture()
            ),
            Some(Refusal {
                code: REFUSED_CODE.to_owned(),
                message: String::new(),
            }),
            "the reader walks every block and gives the first refusal"
        );
    }

    #[test]
    fn a_refusal_that_names_another_picture_is_no_refusal_of_this_one() {
        // The answer of a terminal reaches whoever reads that terminal next.
        // The run before this one is answered late, and a second program that
        // draws kitty pictures on the same terminal writes an answer of its
        // own. Both name another image number, and a reader that reported them
        // would fail a picture that drew.
        assert_eq!(
            read_refusal(
                b"\x1b_Gi=31,I=999999;ENOSPC:the image store is full\x1b\\\x1b[?62;4c",
                this_picture()
            ),
            None,
            "a refusal of image number 999999 says nothing about picture {THIS_PICTURE_NUMBER}"
        );
    }

    #[test]
    fn a_refusal_that_names_this_picture_arrives() {
        // The other half of the rule. A reader that refused every numbered
        // refusal would report nothing at all, which is the defect of issue
        // #465.
        assert_eq!(
            read_refusal(
                b"\x1b_Gi=31,I=7;ENOSPC:the image store is full\x1b\\\x1b[?62;4c",
                this_picture()
            ),
            Some(Refusal {
                code: REFUSED_CODE.to_owned(),
                message: REFUSED_DETAIL.to_owned(),
            })
        );
    }

    #[test]
    fn a_refusal_that_names_no_picture_arrives() {
        // A terminal that reports a failure and echoes no image number is
        // still a terminal that refused the picture of this run. A strict rule
        // would take that report away, and the user would then stand in front
        // of an empty screen with no word about why.
        assert_eq!(
            read_refusal(b"\x1b_G;ENOSPC\x1b\\\x1b[?62;4c", this_picture()),
            Some(Refusal {
                code: REFUSED_CODE.to_owned(),
                message: String::new(),
            }),
            "a refusal with no image number could be the refusal of this picture"
        );
    }

    #[test]
    fn an_image_number_is_read_whole() {
        // A reader that compared the first character of the value would take
        // 12 for 1. The picture of this test is number 1, and the answer names
        // number 12.
        let first_picture = ImageNumber::new(1).expect("one is above zero");
        assert_eq!(
            read_refusal(b"\x1b_Gi=31,I=12;ENOSPC\x1b\\\x1b[?62;4c", first_picture),
            None,
            "image number 12 is another picture than image number 1"
        );
    }

    #[test]
    fn the_image_id_of_an_answer_names_no_picture() {
        // The two keys are a capital `I` for an image number and a lower case
        // `i` for an image id. A terminal picks the id itself for a picture
        // that named none, so an id says nothing about which picture of this
        // process the terminal answers. A reader of the wrong key would take
        // the refusal below, whose image number names another picture.
        assert_eq!(
            read_refusal(b"\x1b_Gi=7,I=999;ENOSPC\x1b\\\x1b[?62;4c", this_picture()),
            None,
            "the image id of 7 is no image number of 7"
        );
    }

    #[test]
    fn the_refusal_of_this_picture_arrives_behind_the_refusal_of_another() {
        // Both refusals stand in the same read: the late answer of the run
        // before this one, and then the answer of this picture. A reader that
        // stopped at the first refusal it saw would report the wrong code.
        assert_eq!(
            read_refusal(
                b"\x1b_GI=999;ENOSPC\x1b\\\x1b_GI=7;ETOODEEP\x1b\\\x1b[?62;4c",
                this_picture()
            ),
            Some(Refusal {
                code: "ETOODEEP".to_owned(),
                message: String::new(),
            }),
            "the reader walks past the refusal of another picture and reads on"
        );
    }

    #[test]
    fn the_display_of_a_refusal_names_the_code_and_the_detail() {
        // The user reads this line, and it is the whole of what the tool knows
        // about why the picture is missing.
        let refusal = Refusal {
            code: REFUSED_CODE.to_owned(),
            message: REFUSED_DETAIL.to_owned(),
        };
        assert_eq!(refusal.to_string(), "ENOSPC: the image store is full");

        let bare = Refusal {
            code: "ETOODEEP".to_owned(),
            message: String::new(),
        };
        assert_eq!(
            bare.to_string(),
            "ETOODEEP",
            "a refusal with no detail names its code and nothing else"
        );
    }

    /// What the child of the refusal round trip exits with for the refusal
    /// that this test wrote back.
    const NAMED_THE_REFUSAL: libc::c_int = 51;

    /// What it exits with when the probe read no refusal at all.
    const NAMED_NO_REFUSAL: libc::c_int = 53;

    /// What it exits with when the probe read some other refusal.
    const NAMED_ANOTHER_REFUSAL: libc::c_int = 55;

    /// Ask the controlling terminal of this child for a refusal, and exit with
    /// what the probe read.
    ///
    /// This runs in a child of a fork of a test binary that holds many
    /// threads, and it leaves through [`leave`].
    fn report_the_refusal_round_trip(terminal: RawFd, master: RawFd) -> ! {
        claim_the_terminal(terminal);
        // SAFETY: the fork gave this child a copy of the master end, and this
        // child reads nothing of it. The test holds the other copy, and the
        // answer arrives on that one.
        unsafe { libc::close(master) };
        leave(match ask_for_a_refusal(ROUND_TRIP_BUDGET, this_picture()) {
            Some(refusal) => {
                if refusal.code == REFUSED_CODE && refusal.message == REFUSED_DETAIL {
                    NAMED_THE_REFUSAL
                } else {
                    NAMED_ANOTHER_REFUSAL
                }
            }
            None => NAMED_NO_REFUSAL,
        })
    }

    #[test]
    fn the_probe_reads_the_refusal_that_a_terminal_wrote() {
        // The whole round trip, over a terminal that this test answers for.
        // The probe writes the request of the attributes, because every
        // terminal answers that one and its answer is what ends the read. A
        // terminal that refused the picture writes the refusal first, since a
        // terminal answers in the order it reads.
        let (master, slave) = open_a_pseudo_terminal();

        // SAFETY: fork(2) reads nothing of this process. The child leaves
        // through `_exit` alone, so it runs no destructor of this one.
        let child = unsafe { libc::fork() };
        assert!(child != -1, "fork(2) must give a child");
        if child == 0 {
            report_the_refusal_round_trip(slave, master);
        }

        let query = read_the_query(master, REPORT_BUDGET);
        // SAFETY: the buffer is owned here and the length is its own.
        let put =
            unsafe { libc::write(master, REFUSAL_ANSWER.as_ptr().cast(), REFUSAL_ANSWER.len()) };

        let status = wait_for_the_child(child);

        // Both descriptors go back before the assertions, because a failed
        // assertion leaves this test through a panic.
        // SAFETY: this test opened the two descriptors and nothing else holds
        // them.
        unsafe {
            libc::close(slave);
            libc::close(master);
        }

        assert_eq!(
            query, ATTRIBUTES_REQUEST,
            "the probe asks for the attributes alone, and it sends no second picture"
        );
        assert_eq!(
            put,
            isize::try_from(REFUSAL_ANSWER.len())
                .expect("the answer is far below the size of a read"),
            "the whole of the answer reaches the terminal"
        );

        let status = status.expect("the child must report inside the budget");
        assert!(
            libc::WIFEXITED(status),
            "the child must exit, and it exited for a signal instead"
        );
        assert_eq!(
            libc::WEXITSTATUS(status),
            NAMED_THE_REFUSAL,
            "the probe must read the refusal the terminal wrote. \
             {NAMED_NO_REFUSAL} says it read no refusal at all, \
             {NAMED_ANOTHER_REFUSAL} says it read another one, and {FAILED} \
             says a call of the child failed"
        );
    }
}
