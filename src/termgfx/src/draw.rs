//! The one entrance that puts an image on a terminal.
//!
//! A caller gives this module an image and a budget in character cells, and the
//! module writes the image or it reports that the terminal draws none. The
//! choice of protocol, the base64, the escape sequences and the position of the
//! cursor all stay inside.
//!
//! # Why the entrance is one call
//!
//! Three inline-image protocols are in service, and each of them wants the
//! image in a different shape. Kitty takes the image in base64, in chunks of a
//! fixed size, under a list of keys. iTerm2 takes a whole image file in
//! base64, in one escape sequence, with the size in character cells. Sixel
//! takes a palette and then a band of pixels at a time, at a size in pixels,
//! from an encoder. A caller that picked the protocol itself would then hold
//! three shapes of the same picture, and every tool that draws would hold the
//! same three.
//!
//! `ic` held them, and it held them as five functions that reached for
//! `io::stdout()` on their own. That is the second reason for one entrance: a
//! writer that locks standard output writes its bytes when it wants to, and a
//! caller that builds a whole frame in a buffer and sends it in one `write(2)`
//! cannot use such a writer. `krt` builds such a frame. So the writers take a
//! stream, the caller says which stream, and `ic` hands them the same locked
//! standard output that it had before.

use std::borrow::Cow;
use std::io::{self, Write};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::prelude::{Engine, BASE64_STANDARD};
use icy_sixel::{sixel_encode, EncodeOptions};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ExtendedColorType, ImageEncoder};

use crate::cursor::{write_image_with_cursor_contract, CursorContract};
use crate::detect::{Capabilities, DisplayRoutine};
use crate::geometry::{
    calculate_aspect_preserving_size, calculate_sixel_dimensions, cell_aspect_of,
    cell_pixels_or_estimate_of, cells_of, downscale_to_display_pixels, image_rows,
    image_rows_in_cells, sixel_pixel_budget, window_pixels, CellPixels,
};
use crate::probe::{ask_for_a_refusal, Refusal, QUERY_BUDGET};

/// The number of base64 characters that one Kitty graphics command carries.
///
/// The protocol limits the size of one command, and the documentation of Kitty
/// names 4096 as the size that a client sends. A larger chunk risks a renderer
/// that drops the command, and a smaller chunk only adds escape sequences
/// around the same pixels.
const KITTY_CHUNK_SIZE: usize = 4096;

/// The Kitty graphics command that takes every image off the screen.
///
/// `a=d` is the delete action and `d=A` names every placement of every image.
/// The upper case `d` value frees the pixels of the image as well, where the
/// lower case value keeps them in the memory of the renderer for a later
/// placement. A caller that draws a new image for each frame never places an
/// old one again, so it frees the pixels.
const KITTY_DELETE_ALL: &str = "\x1b_Ga=d,d=A\x1b\\";

/// The Kitty graphics key that stops the terminal from answering a command.
///
/// A Kitty terminal answers an image command that names an image id, and the
/// answer is an APC sequence on the terminal itself. [`Picture::Frame`] wants
/// no answer, because the caller that draws one frame after another holds the
/// terminal in raw mode for the key presses of the user: the answer arrives at
/// that caller as key presses, and `krt` then reads the `p` of `i=1,p=1;OK` as
/// the pause command of its own live table. `q=2` takes the success answer and
/// the failure answer both away.
///
/// [`Picture::Still`] asks for the failures instead, through
/// [`KITTY_FAILURES_ONLY`], because a caller that draws one picture reads the
/// answer and then gives the terminal back to the shell.
///
/// [`KITTY_DELETE_ALL`] carries no such key, because it names no image id and a
/// Kitty terminal answers it never.
const KITTY_QUIET: &str = "q=2";

/// The Kitty graphics key that asks the terminal for the failures alone.
///
/// `q=1` takes the success answer away and leaves the failure answer.
/// [`Picture::Still`] wants that one. A terminal that refuses the picture draws
/// nothing, and the tool that asked for no answer then reports success in front
/// of an empty screen. The image store of a mosh session holds a fixed number of
/// bytes and refuses a picture above it, so the case is a common one.
///
/// **The caller of this crate reads the answer that this key asks for.** A
/// caller that asks a terminal for a failure report and then reads nothing
/// leaves that report on the descriptor the shell of the user reads next, and
/// the shell takes the bytes of it for key presses.
/// [`Capabilities::read_refusal`] is the read.
const KITTY_FAILURES_ONLY: &str = "q=1";

/// The name of the Kitty graphics key that carries an image number.
///
/// **A Kitty terminal answers a transmission only when the transmission names
/// an image id or an image number.** The specification says of the `i` key that
/// the terminal replies after it tried to load the image, and the parser of
/// Ghostty states the same rule as one line of code: a transmission that names
/// neither key gets no answer at all. So [`KITTY_FAILURES_ONLY`] reports
/// nothing without a key such as this one beside it.
///
/// The name is `I` and not `i`, because the two mean different things. An `i`
/// is an image id, and the specification says that a re-transmission of an id
/// deletes the image which held that id and every placement of that image. One
/// fixed id here would therefore take the picture of `ic a.png` off the screen
/// the moment `ic b.png` drew. An `I` is an image number, and the specification
/// gives it for exactly this case: a new image arrives even when an image of
/// the same number stands already, and the terminal answers with the id that it
/// made. Kitty, Ghostty and WezTerm all read it.
///
/// **The value of the key differs for every still picture**, and
/// [`ImageNumber`] states why. The `probe` module reads this same name out of
/// the answer of a terminal, so the name stands here one time and the writer
/// and the reader cannot part company over it.
pub(crate) const KITTY_IMAGE_NUMBER_KEY: &str = "I";

/// The image number of one still picture.
///
/// A Kitty terminal names the picture it speaks about by the image number that
/// the picture carried, so this number is what tells a refusal of this picture
/// from a refusal of another one. Every still picture carried `I=1` before, and
/// one number gives every picture the same name: the late answer of the run
/// before, and the answer of a second program that draws Kitty pictures on the
/// same terminal, both name this picture as well. `ic` then reported a refusal
/// for a picture that drew, and a false failure over a good picture is worse
/// than the silence of issue #465.
///
/// So the number differs for every still picture, in four directions:
///
/// * A counter takes one step for each picture, so two pictures of one run
///   carry two numbers.
/// * The counter starts at a seed of the process id, so two runs on one machine
///   start at two numbers.
/// * The seed also carries the nanoseconds of the clock, so two runs that a
///   recycled process id gives one name still start apart.
/// * Every number stands above [`IMAGE_NUMBER_FLOOR`], so a program that counts
///   its pictures from one picks no number of this run at all.
///
/// The seed is a guess of two runs apart, and no promise of it. Two runs that
/// pick one seed carry the misattribution that this number takes away, at the
/// rate that the seed repeats. The floor is a promise, because a number below
/// it leaves this crate never.
///
/// The value is never zero, because zero names no image to a Kitty terminal.
/// The type carries that rule, so no caller of it states the rule again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ImageNumber(NonZeroU32);

impl ImageNumber {
    /// The image number `raw`, or [`None`] for zero, which names no image.
    ///
    /// # Arguments
    /// * `raw` - The number that a terminal named, or that a caller kept.
    ///
    /// # Returns
    /// The number, for every value above zero.
    pub(crate) fn new(raw: u32) -> Option<Self> {
        NonZeroU32::new(raw).map(Self)
    }

    /// The number as a plain number.
    ///
    /// A caller that keeps the number between two calls keeps this value.
    pub(crate) fn get(self) -> u32 {
        self.0.get()
    }

    /// The number of the next still picture of this process.
    ///
    /// Each call gives a number of its own, and the numbers of one run repeat
    /// after 1073741824 still pictures.
    fn mint() -> Self {
        let taken = still_picture_counter().fetch_add(1, Ordering::Relaxed);
        // The step of the counter reaches the top of a 32-bit number and wraps
        // there, so the number of the counter alone falls below the floor and
        // to zero. The low bits of it stand inside the range instead, and the
        // bit of the floor holds every number of this crate above zero as well.
        Self(IMAGE_NUMBER_FLOOR | (taken & (IMAGE_NUMBER_FLOOR.get() - 1)))
    }
}

impl std::fmt::Display for ImageNumber {
    /// Write the number, which is the value of [`KITTY_IMAGE_NUMBER_KEY`].
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}", self.0)
    }
}

/// How far the seed turns the process id.
///
/// The seed carries the process id and the nanoseconds of the clock, and
/// [`ImageNumber::mint`] takes the low 30 bits of it. The nanoseconds fill
/// those 30 bits, so a process id that stood beside them in the low bits would
/// meet them there. The turn moves the process id up, and it still holds the
/// low 17 bits of that id inside the 30 bits that a picture carries. Those low
/// bits are the bits that two runs of one machine differ in.
const PROCESS_ID_TURN: u32 = 13;

/// The floor that every image number of this crate stands above.
///
/// A program that counts its pictures from one picks small numbers, and a
/// number above this floor reaches none of them. So a refusal of such a program
/// names no picture of this run, whatever the seed of this run came to.
///
/// The floor is the second bit from the top, and no number of this crate
/// reaches the top bit. A Kitty image number is a 32-bit unsigned number, and a
/// terminal that reads one into a signed number of the same width still holds
/// every number of this range. The 30 bits below the floor carry the seed, so
/// a run has 1073741824 numbers of its own to pick from.
pub(crate) const IMAGE_NUMBER_FLOOR: NonZeroU32 =
    NonZeroU32::new(1 << 30).expect("one bit of a number is above zero");

/// The counter that [`ImageNumber::mint`] steps.
///
/// It starts one time for each run, at a number of the process id and the
/// clock. A counter that started at a fixed number would give the first picture
/// of every run one name, and that name is what a late answer of the run before
/// carries. [`ImageNumber::mint`] reads the low bits of this counter alone, so
/// the high bits of the seed reach no picture and the seed states no range of
/// its own.
fn still_picture_counter() -> &'static AtomicU32 {
    static COUNTER: OnceLock<AtomicU32> = OnceLock::new();
    COUNTER.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.subsec_nanos());
        AtomicU32::new(std::process::id().rotate_left(PROCESS_ID_TURN) ^ nanos)
    })
}

/// What [`LAST_STILL_IMAGE_NUMBER`] holds before a still picture goes out.
///
/// Zero names no image to a Kitty terminal, so no picture carries it and no
/// answer of a terminal names it.
const NO_STILL_PICTURE: u32 = 0;

/// The image number that the last still picture of this process carried.
///
/// [`Capabilities::read_refusal`] reads it. The number leaves in the picture,
/// and the answer of the terminal names it, so the reader of that answer needs
/// the number that the writer sent. The two stand in two calls, and
/// [`Capabilities::read_refusal`] states why, so the number waits here between
/// them.
///
/// One cell holds one number, and the read is of the picture that went before,
/// which is the contract that [`Capabilities::read_refusal`] states. A process
/// that draws two still pictures and then reads asks about the second one.
static LAST_STILL_IMAGE_NUMBER: AtomicU32 = AtomicU32::new(NO_STILL_PICTURE);

/// Mint the image number of a still picture and keep it for the reader of the
/// answer.
///
/// # Returns
/// The number that the picture carries, which the caller writes behind
/// [`KITTY_IMAGE_NUMBER_KEY`].
fn number_of_a_still_picture() -> ImageNumber {
    let number = ImageNumber::mint();
    LAST_STILL_IMAGE_NUMBER.store(number.get(), Ordering::Relaxed);
    number
}

/// The Kitty graphics key that stops the renderer from moving the cursor.
///
/// The writer states the position of the cursor itself, through
/// [`write_image_with_cursor_contract`]. A renderer that also moved the cursor
/// would double the movement.
///
/// Every image carries this key, whatever [`Request::cursor`] asks for, because
/// the writer owns the movement in both contracts. [`Cursor::Held`] wants no
/// movement at all, and [`Cursor::BelowImage`] wants the movement that the
/// contract writes around the payload.
const KITTY_HOLD_CURSOR: &str = "C=1";

/// How much of the terminal one image can take, in character cells.
///
/// An axis is `None` when the caller states no bound on it. The protocols each
/// have a rule for that case, and they agree: an image with one axis bound
/// keeps its aspect ratio inside that axis, and an image with neither axis
/// bound draws at its own pixel size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The width in character cells, when the caller states one.
    pub columns: Option<u32>,
    /// The height in character cells, when the caller states one.
    pub rows: Option<u32>,
}

/// The characters of payload that one image can spend.
///
/// A terminal that carries an image over a network caps what one image can
/// spend, and a transmission above that cap draws nothing at all. mosh is the
/// cap that matters in practice, and it bounds each of the three protocols
/// that this module writes. [`PayloadBudget::MOSH`] names those three caps and
/// says where each one stands.
///
/// The budget bounds the payload that the protocol carries, and the keys in
/// front of that payload are a few tens of characters. [`PayloadBudget::MOSH`]
/// leaves room for them.
///
/// A picture above the budget is drawn at fewer pixels rather than not at all.
/// The Kitty protocol and the iTerm2 protocol each state the size of the
/// picture in character cells, so a smaller pixel count keeps the size that the
/// picture takes on the screen and loses resolution alone. The Sixel protocol
/// states its size in pixels and carries no such key, so a Sixel picture that
/// spends fewer pixels is smaller on the screen as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayloadBudget(usize);

impl PayloadBudget {
    /// The room that [`PayloadBudget::MOSH`] leaves for the keys of the
    /// command.
    ///
    /// mosh counts the keys and the payload of one command together, so the
    /// keys come out of the same mebicharacter that the payload spends. A
    /// Kitty control block runs to about eighty characters, and the arguments
    /// of the other two protocols are shorter. This room stands far above all
    /// three, because a picture that loses four kibicharacters of resolution
    /// loses nothing a reader can see.
    const CONTROL_BLOCK_ROOM: usize = 4096;

    /// The budget of a mosh session.
    ///
    /// mosh caps one image at one mebicharacter, and the cap is the same
    /// number for each of the three protocols that this module writes. The
    /// three numbers stand in `timmattison/mosh-rs` at commit `5676142`
    /// (<https://github.com/timmattison/mosh-rs>):
    ///
    /// * Kitty: `MAXIMUM_STORED_CHARACTERS` of
    ///   `crates/mosh-terminal/src/imagestore.rs`. `ImageStore::hold` refuses a
    ///   transmission above it with [`crate::Refusal`] `ENOSPC`, and it counts
    ///   `control.len() + payload.len()`. The store holds every image it
    ///   accepted under that same number for the length of the session, and it
    ///   evicts the oldest images to make room for a new one.
    /// * iTerm2: `MAXIMUM_INLINE_IMAGE_CHARACTERS` of
    ///   `crates/mosh-terminal/src/dispatcher.rs`. The protocol carries a whole
    ///   image in one operating system command, and mosh drops every character
    ///   of that command above the cap.
    /// * Sixel: `MAXIMUM_SIXEL_STRING_CHARACTERS` of the same file, which
    ///   bounds one device-control string in the same way.
    pub const MOSH: Self = Self(1024 * 1024 - Self::CONTROL_BLOCK_ROOM);

    /// The budget of a terminal that states no cap of its own.
    pub const UNLIMITED: Self = Self(usize::MAX);

    /// A budget of `characters`.
    ///
    /// # Arguments
    /// * `characters` - The characters of payload that one image can spend.
    #[must_use]
    pub const fn of(characters: usize) -> Self {
        Self(characters)
    }

    /// Whether a payload of `characters` fits inside this budget.
    ///
    /// # Arguments
    /// * `characters` - The characters that the encoder made.
    #[must_use]
    const fn holds(self, characters: usize) -> bool {
        characters <= self.0
    }

    /// The characters that this budget allows.
    #[must_use]
    const fn characters(self) -> usize {
        self.0
    }
}

/// Where the cursor stands when the image is written.
///
/// No image protocol promises a position of the cursor, and each renderer
/// decides for itself, so the caller states the position it wants instead of a
/// guess.
///
/// This states the cursor and it states nothing else. [`Picture`] states
/// whether the run draws one picture or one frame of many, and the two answers
/// are free of each other: a user who types `ic -n photo.png` holds the cursor
/// for one still picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// The crate writes the image and moves the cursor to the row under it.
    BelowImage,
    /// The caller holds the cursor, and the crate moves nothing.
    Held,
}

/// Whether the run draws one picture or one frame of many.
///
/// A run that draws one picture and a run that draws frame after frame want
/// two different things from the same protocol, and the difference is the cost
/// that each of them pays.
///
/// A still picture pays its characters one time, so it travels in the shape
/// that costs the fewest of them, and it asks the terminal for the failures
/// because the caller reads that answer before it gives the terminal back to
/// the shell. A frame pays for every frame, so it takes the shape that costs
/// the least time, and it asks for no answer at all because the caller holds
/// the terminal in raw mode for the key presses of the user.
///
/// This is a different question from [`Cursor`], and the two answers are free
/// of each other. A user who types `ic -n photo.png` draws one still picture
/// and holds the cursor as well.
///
/// The choice means something to the Kitty graphics protocol alone. The Sixel
/// protocol and the iTerm2 protocol each carry one shape of an image and answer
/// no command at all, so the two writers of those protocols read this and then
/// ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    /// One still picture. It travels as a whole PNG file, and it asks the
    /// terminal for the failures.
    Still,
    /// One frame of many. It keeps the raw pixels, it asks the terminal for no
    /// answer, and it carries the placement `id` that makes the next frame of
    /// that same id replace it in place instead of stand beside it.
    Frame {
        /// The placement id of the frame, which a Kitty terminal reads and the
        /// other two protocols ignore.
        id: u32,
    },
}

/// One image, and what the caller asks the terminal to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// How much of the terminal the image can take.
    pub budget: Budget,
    /// How many characters of payload the image can spend.
    pub payload: PayloadBudget,
    /// Whether the run draws one picture or one frame of many.
    pub picture: Picture,
    /// Where the cursor stands when the image is written.
    pub cursor: Cursor,
    /// True when the image keeps its aspect ratio inside the budget.
    pub preserve_aspect: bool,
}

/// The reason that no image reached the terminal.
#[derive(Debug, thiserror::Error)]
pub enum DrawError {
    /// The terminal draws no inline image at all.
    #[error("this terminal draws no inline image")]
    NoGraphics,
    /// A write to the stream failed.
    #[error(transparent)]
    Write(#[from] io::Error),
    /// The encoder refused the image.
    #[error("the encoder refused the image: {0}")]
    Encode(String),
}

impl Capabilities {
    /// Write `image` into `out`, inside `request.budget`.
    ///
    /// This is the one entrance. It picks the protocol that this terminal
    /// reads, puts the image into the shape that protocol wants, keeps the
    /// promise of `request.cursor`, and flushes `out`.
    ///
    /// The stream comes from the caller, and this call never reaches for
    /// standard output on its own. A caller that draws one image at a prompt
    /// hands over a locked standard output. A caller that builds a whole frame
    /// hands over the buffer of that frame, and the frame then leaves in one
    /// write.
    ///
    /// # Arguments
    /// * `out` - The stream that takes the bytes.
    /// * `image` - The image to draw.
    /// * `request` - The budget, the cursor and the aspect ratio.
    ///
    /// # Errors
    /// Gives [`DrawError::NoGraphics`] when this terminal draws no inline image
    /// at all, and then leaves `out` untouched. Every protocol carries its
    /// image in an escape sequence, and a terminal that reads none of the three
    /// puts the sequence on the screen as text, so the answer has to come back
    /// before one byte leaves. Gives [`DrawError::Encode`] when the encoder of
    /// the protocol refuses the image, and [`DrawError::Write`] when a write to
    /// `out` fails.
    pub fn draw<W: Write>(
        &self,
        out: &mut W,
        image: &DynamicImage,
        request: &Request,
    ) -> Result<(), DrawError> {
        if !self.draws_images() {
            return Err(DrawError::NoGraphics);
        }

        // The cell that the terminal named travels with the request, because
        // the one read of the terminal happened before this call and no writer
        // reads the terminal a second time.
        let answered = self.answered_cell();
        match self.display_routine() {
            DisplayRoutine::Sixel => write_sixel(out, image, request, answered),
            DisplayRoutine::Kitty => write_kitty(out, image, request, answered),
            DisplayRoutine::Iterm2 => write_iterm2(out, image, request, answered),
        }?;

        out.flush()?;
        Ok(())
    }

    /// Read the refusal that this terminal wrote for the picture that went
    /// before.
    ///
    /// A Kitty terminal answers a picture that it refused, and it names a code
    /// such as `ENOSPC` in that answer. A caller that reads the answer tells
    /// the user why the screen is empty. A caller that reads none reports that
    /// it drew a picture that never arrived.
    ///
    /// # Why this is a second call
    ///
    /// [`Capabilities::draw`] writes bytes and it reads none. A caller that
    /// holds the terminal in raw mode reads that terminal itself, and `krt` is
    /// such a caller, so a read inside `draw` would take a key press of the
    /// user out of its hands. A test of the writer would reach the terminal of
    /// whoever runs the suite as well, where it reaches a buffer today. So the
    /// read stands in a call of its own, and the caller that wants the answer
    /// asks for it.
    ///
    /// # The caller reads what it asked for
    ///
    /// Call this after a still picture. **A caller that asks a terminal for a
    /// failure report and then reads nothing leaves that report on the
    /// descriptor the shell of the user reads next, and the shell takes the
    /// bytes of it for key presses.** A caller that draws one frame after
    /// another calls this never: the answer of a terminal costs a round trip,
    /// and a round trip for each frame stands inside the frame loop.
    ///
    /// The question goes to the controlling terminal, which is a descriptor of
    /// its own. It does not go to the stream that took the picture, because
    /// that stream is a buffer or a file for many callers and neither one
    /// answers anything.
    ///
    /// # Which picture the refusal is about
    ///
    /// The answer of a terminal reaches whoever reads that terminal next, and
    /// that reader is this call. So the bytes it reads hold the answer of the
    /// picture that went before, and they also hold every answer that arrived
    /// late from the run before and from a second program that draws Kitty
    /// pictures on the same terminal.
    ///
    /// The image number tells them apart. The picture that went before carries
    /// the number of [`ImageNumber`], which is a number of its own, and this
    /// call takes that number out of [`LAST_STILL_IMAGE_NUMBER`] and gives it
    /// to the reader of the answer.
    ///
    /// # Returns
    /// The refusal that the terminal reported for the picture that went before,
    /// or [`None`]. [`None`] covers a picture that drew, a terminal that
    /// reports nothing, a terminal of the Sixel protocol or the iTerm2
    /// protocol, which answer no command at all, a run that owns no terminal to
    /// ask, a run that drew no still picture at all, and a refusal that names
    /// another picture.
    #[must_use]
    pub fn read_refusal(&self) -> Option<Refusal> {
        if self.display_routine() != DisplayRoutine::Kitty {
            return None;
        }

        let number = ImageNumber::new(LAST_STILL_IMAGE_NUMBER.load(Ordering::Relaxed))?;
        ask_for_a_refusal(QUERY_BUDGET, number)
    }

    /// Take every image that this crate placed off the screen.
    ///
    /// The call writes the delete command of the Kitty graphics protocol when
    /// the terminal reads that protocol, and it writes nothing at all for the
    /// Sixel protocol and the iTerm2 protocol. Those two paint their pixels
    /// into the screen and keep no handle on them, so a clear of the screen
    /// already takes them off. Kitty keeps a placement instead, and a placement
    /// outlives a clear of the screen, so a caller that draws one image for
    /// each frame stacks the frames of the whole run on the screen unless it
    /// deletes them.
    ///
    /// `krt` calls this at the head of every frame. `ic` draws one image and
    /// then gives the terminal back to the shell, so it calls this never.
    ///
    /// The Kitty half is the half that a test of this crate can read, because
    /// it writes bytes. The Sixel half and the iTerm2 half write nothing, and
    /// what they promise is a screen with no image left on it. Only a real
    /// terminal shows that, so the user of issue #393 is the one who tests
    /// those two halves.
    ///
    /// # Arguments
    /// * `out` - The stream that takes the bytes.
    ///
    /// # Errors
    /// Gives the error of the write to `out` when the write fails.
    pub fn clear_images<W: Write>(&self, out: &mut W) -> io::Result<()> {
        if self.display_routine() == DisplayRoutine::Kitty {
            write!(out, "{KITTY_DELETE_ALL}")?;
        }

        Ok(())
    }
}

/// Give the cursor contract that one request asks for.
///
/// [`Cursor::Held`] is the caller-managed contract, because a caller that holds
/// the cursor is the caller that puts the cursor where it wants it.
///
/// # Arguments
/// * `request` - The request that names the cursor.
/// * `term_rows` - The height of the terminal in rows, off the one window that
///   the writer measured. [`CursorContract::below_image`] bounds the
///   reservation by it, so the picture and the reservation below it name one
///   terminal.
/// * `image_rows` - Gives the height of the image in terminal rows. It runs
///   only for [`Cursor::BelowImage`]. A caller that draws frame after frame
///   asks for [`Cursor::Held`] one time for each frame, so that arithmetic
///   never stands on its path.
///
/// # Returns
/// The promise that the writer must keep.
fn cursor_contract(
    request: &Request,
    term_rows: u32,
    image_rows: impl FnOnce() -> u32,
) -> CursorContract {
    CursorContract::below_image(
        matches!(request.cursor, Cursor::Held),
        term_rows,
        image_rows,
    )
}

/// One shape that a picture travels in, and the cheaper shape under it.
///
/// A protocol states which shapes it carries, and the shapes of one protocol
/// stand in an order: each one costs fewer characters of the same pixels than
/// the one above it, and it pays for them with something else.
/// [`fit_to_payload_budget`] walks that order before it takes a pixel off the
/// picture, because the pixel count is what the reader sees.
///
/// The Kitty protocol and the Sixel protocol each carry one shape, so
/// [`Payload::cheaper`] answers [`None`] for them and the fit reaches for the
/// pixels at once. The iTerm2 protocol carries a whole file of any format the
/// terminal reads, so [`Iterm2Payload`] states a real order there.
trait Payload: Copy {
    /// Encode `image` into the base64 payload of this shape.
    ///
    /// # Arguments
    /// * `image` - The image at the size that it draws at.
    ///
    /// # Errors
    /// Gives [`DrawError::Encode`] when the encoder refuses the image.
    fn encode(self, image: &DynamicImage) -> Result<String, DrawError>;

    /// The shape that carries the same pixels for fewer characters, or [`None`]
    /// when this shape is the last one that the protocol carries.
    fn cheaper(self) -> Option<Self>;
}

/// The shape that one Kitty image travels in.
///
/// The protocol takes either the raw pixels of an image or a whole image file,
/// and the two cost very different numbers of characters. Base64 turns three
/// bytes into four characters, and three bytes is one pixel, so raw pixels cost
/// four characters for every pixel: 580800 characters for a photograph of 330
/// pixels by 440. A mosh session holds 1048576 characters of image, so one such
/// picture takes over half of that store. A photograph of twice the pixels
/// costs more than the whole store, and it never arrives. A PNG of the same
/// photograph costs a fraction of it.
///
/// The variant owns the `f=` key, the keys that state the pixel size, and the
/// encoder, all three together. One place therefore decides the header and the
/// payload, and the two cannot name different shapes.
///
/// Both shapes drop the alpha channel. The drawn result is the same, and three
/// bytes for one pixel is a smaller payload than four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KittyPayload {
    /// The raw pixels, three bytes for one pixel, under `f=24`. The pixels
    /// state no size of their own, so the header states it beside them.
    RawRgb,
    /// A whole PNG file, under `f=100`. A Kitty terminal reads the width and
    /// the height out of the file, so the header states neither.
    Png,
}

impl KittyPayload {
    /// Give the value of the `f=` key that names this shape to the terminal.
    ///
    /// # Returns
    /// The value of the key, with no key name and no comma.
    fn format_key(self) -> &'static str {
        match self {
            KittyPayload::RawRgb => "24",
            KittyPayload::Png => "100",
        }
    }

    /// Give the header keys that state the pixel size of the image.
    ///
    /// A PNG carries its own width and height, and the documentation of Kitty
    /// says that a terminal reads them out of the file. An `s=` key or a `v=`
    /// key beside a PNG is therefore a second statement of one fact, which is a
    /// second statement that can disagree.
    ///
    /// # Arguments
    /// * `width` - The width of the image in pixels.
    /// * `height` - The height of the image in pixels.
    ///
    /// # Returns
    /// The keys, with the comma that joins them to the header before them, for
    /// raw pixels. Nothing at all for a PNG.
    fn pixel_size_keys(self, width: u32, height: u32) -> String {
        match self {
            KittyPayload::RawRgb => format!(",s={width},v={height}"),
            KittyPayload::Png => String::new(),
        }
    }

}

impl Payload for KittyPayload {
    /// Encode `image` into the base64 payload of this shape.
    ///
    /// The PNG goes out at the default compression of the encoder and not at
    /// the strongest one. A still picture must appear at once, and the
    /// strongest compression spends seconds of a large picture to save a few
    /// characters of it.
    ///
    /// # Arguments
    /// * `image` - The image at the size that it draws at.
    ///
    /// # Errors
    /// Gives [`DrawError::Encode`] when the PNG encoder refuses the image.
    fn encode(self, image: &DynamicImage) -> Result<String, DrawError> {
        // Both shapes start from RGB8. The alpha channel changes no pixel that
        // a terminal draws, and it makes the payload one third larger.
        let rgb = image.to_rgb8();

        match self {
            KittyPayload::RawRgb => Ok(BASE64_STANDARD.encode(rgb.as_raw())),
            KittyPayload::Png => {
                let mut file = Vec::new();
                PngEncoder::new(&mut file)
                    .write_image(
                        rgb.as_raw(),
                        rgb.width(),
                        rgb.height(),
                        ExtendedColorType::Rgb8,
                    )
                    .map_err(|error| DrawError::Encode(error.to_string()))?;

                Ok(BASE64_STANDARD.encode(&file))
            }
        }
    }

    /// The Kitty protocol carries these two shapes and no third one, and
    /// [`write_kitty`] picks between them by what the caller draws rather than
    /// by what the budget holds. A still picture already travels as a PNG, and
    /// raw pixels are what a frame trades characters for time with, so neither
    /// shape has a cheaper one under it.
    fn cheaper(self) -> Option<Self> {
        None
    }
}

/// The one shape that a Sixel image travels in.
///
/// The protocol carries a palette and then a band of pixels at a time, and the
/// encoder of `icy_sixel` makes both. It names no second shape, so this type
/// holds no data: it is the encoder under the name that
/// [`fit_to_payload_budget`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SixelPayload;

impl Payload for SixelPayload {
    /// Encode `image` into the device control string that a Sixel terminal
    /// reads.
    ///
    /// The string is the payload and the command together, because the encoder
    /// writes the introducer and the terminator itself. So the writer sends
    /// what this gives it and adds nothing.
    ///
    /// # Arguments
    /// * `image` - The image at the size that it draws at.
    ///
    /// # Errors
    /// Gives [`DrawError::Encode`] when the encoder refuses the image.
    fn encode(self, image: &DynamicImage) -> Result<String, DrawError> {
        let rgba = image.to_rgba8();

        sixel_encode(
            rgba.as_raw(),
            image.width() as usize,
            image.height() as usize,
            &EncodeOptions::default(),
        )
        .map_err(|error| DrawError::Encode(error.to_string()))
    }

    /// The protocol carries this shape and no other one, so a Sixel picture
    /// that stands above the budget reaches it on pixels alone.
    fn cheaper(self) -> Option<Self> {
        None
    }
}

/// The quality that a JPEG encoder works at, from 1 to 100.
///
/// The quality decides how much of the picture the encoder throws away, and it
/// is the thing that [`Iterm2Payload`] spends before it spends a pixel. The
/// rungs run from [`JpegQuality::HIGHEST`] down to [`JpegQuality::LOWEST`], a
/// step of [`JpegQuality::STEP`] at a time, and [`JpegQuality::cheaper`] is the
/// one place that walks them.
///
/// The type carries the range, so no caller of the encoder states it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JpegQuality(u8);

impl JpegQuality {
    /// The quality that a JPEG starts at.
    ///
    /// A photograph at this quality is about a third of the same photograph as
    /// a PNG and it holds every pixel, so it is the first rung that
    /// [`Iterm2Payload::cheaper`] steps onto. A higher number buys a difference
    /// that no reader of a terminal sees, and it costs characters that the
    /// lower rungs then have to find again.
    const HIGHEST: Self = Self(90);

    /// The quality that the ladder stops at.
    ///
    /// The report of this defect measures a photograph of 3074 pixels by 1856
    /// at this quality: 785138 bytes, which is what a mosh session holds. So
    /// this rung carries the whole picture where the rung above it cannot.
    /// Under it the blocks of the encoder start to show, and the pixel count is
    /// then the better thing to spend.
    const LOWEST: Self = Self(35);

    /// How far one step of the ladder falls.
    ///
    /// The distance from [`JpegQuality::HIGHEST`] to [`JpegQuality::LOWEST`]
    /// divides by this, so the ladder lands on the lowest rung exactly and
    /// spends six encoder runs to get there.
    const STEP: u8 = 11;

    /// The rung under this one, or [`None`] at [`JpegQuality::LOWEST`].
    ///
    /// # Returns
    /// The next rung down. The step never falls under the lowest rung, so a
    /// change of [`JpegQuality::STEP`] that no longer divides the ladder
    /// evenly still stops there.
    fn cheaper(self) -> Option<Self> {
        (self.0 > Self::LOWEST.0)
            .then(|| Self(self.0.saturating_sub(Self::STEP).max(Self::LOWEST.0)))
    }

    /// The quality as the number that the encoder takes.
    fn get(self) -> u8 {
        self.0
    }
}

/// The shape that one iTerm2 image travels in.
///
/// The protocol carries a whole file, and the terminal reads the format out of
/// the first bytes of that file. So this writer picks any format the terminal
/// draws, and it costs no key of the protocol and no round trip to say which
/// one it picked.
///
/// The shapes stand in one order, and [`fit_to_payload_budget`] walks it:
///
/// * [`Iterm2Payload::Png`] first. It is lossless, so a picture that the budget
///   holds as a PNG reaches the terminal with every pixel that the caller gave
///   it. A picture of flat color and sharp edges, such as a screenshot of text,
///   also costs less as a PNG than as a JPEG at any quality.
/// * [`Iterm2Payload::Jpeg`] under it, one quality at a time. A photograph
///   compresses poorly in a lossless format, and a JPEG of it carries about
///   twelve times the pixels of a PNG for the same characters. That is what
///   keeps a photograph at the resolution of the screen inside the budget of a
///   mosh session.
///
/// The writer made a raw PNM file before this: three bytes for every pixel and
/// no compression at all. A photograph of 3074 pixels by 1856 costs 22821376
/// base64 characters that way, and a mosh session holds 1048576, so the fit
/// shrank the picture to about 655 pixels by 395 and the terminal stretched
/// that over the whole rectangle.
///
/// Both shapes drop the alpha channel. A JPEG carries none at all, and the PNM
/// file that went before carried none either, so the drawn result is the one
/// that the writer drew before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Iterm2Payload {
    /// A whole PNG file, which loses no pixel of the picture.
    Png,
    /// A whole JPEG file at this quality.
    Jpeg(JpegQuality),
}

impl Payload for Iterm2Payload {
    /// Encode `image` into the base64 payload of this shape.
    ///
    /// The PNG goes out at the default compression of the encoder and not at
    /// the strongest one, for the reason that [`KittyPayload::encode`] gives: a
    /// still picture must appear at once.
    ///
    /// # Arguments
    /// * `image` - The image at the size that it draws at.
    ///
    /// # Errors
    /// Gives [`DrawError::Encode`] when the encoder refuses the image.
    fn encode(self, image: &DynamicImage) -> Result<String, DrawError> {
        // Both shapes start from RGB8. A JPEG carries no alpha channel at all,
        // and the alpha of a PNG changes no pixel that this writer draws.
        let rgb = image.to_rgb8();
        let mut file = Vec::new();

        match self {
            Iterm2Payload::Png => PngEncoder::new(&mut file).write_image(
                rgb.as_raw(),
                rgb.width(),
                rgb.height(),
                ExtendedColorType::Rgb8,
            ),
            Iterm2Payload::Jpeg(quality) => {
                JpegEncoder::new_with_quality(&mut file, quality.get()).write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    ExtendedColorType::Rgb8,
                )
            }
        }
        .map_err(|error| DrawError::Encode(error.to_string()))?;

        Ok(BASE64_STANDARD.encode(&file))
    }

    /// The shape under this one.
    ///
    /// # Returns
    /// The highest JPEG quality under a PNG, the next rung down under a JPEG,
    /// and [`None`] under [`JpegQuality::LOWEST`], where the fit starts to
    /// spend pixels instead.
    fn cheaper(self) -> Option<Self> {
        match self {
            Iterm2Payload::Png => Some(Iterm2Payload::Jpeg(JpegQuality::HIGHEST)),
            Iterm2Payload::Jpeg(quality) => quality.cheaper().map(Iterm2Payload::Jpeg),
        }
    }
}

/// The resizes that [`fit_to_payload_budget`] takes before it gives up.
///
/// Every resize divides the pixel count by the amount that the last attempt
/// missed by, so a payload that is a hundred times too large reaches the
/// budget in two. Six is far past what any real picture needs, and it bounds
/// the encoder runs of one draw whatever a future encoder does with the size.
///
/// The shapes of a protocol bound themselves, because
/// [`Payload::cheaper`] walks a list that each protocol states and that list
/// ends. So the encoder runs of one fit come to the length of that list plus
/// this number plus one.
const MAXIMUM_FIT_ATTEMPTS: usize = 6;

/// The share of the budget that one attempt of [`fit_to_payload_budget`] aims
/// at.
///
/// A payload is not exactly linear in the pixel count, so an attempt that
/// aimed at the whole budget would land a little above it about half the time
/// and cost a second encoder run. Aiming a little under it costs a few pixels
/// that nobody can see.
const FIT_SAFETY: f64 = 0.95;

/// Carry `image` in `shape`, or in a cheaper shape, or at fewer pixels, until
/// the payload is one that `budget` holds.
///
/// The fit spends two things and it spends them in this order.
///
/// **The shape first.** [`Payload::cheaper`] names the shape under the one the
/// picture is in, and every rung of that ladder carries the same pixels for
/// fewer characters. So a picture that reaches the budget on the ladder alone
/// reaches it at the resolution that the screen shows.
///
/// **The pixel count second, and only when the ladder ends.** The payload of
/// every shape this crate writes grows with the pixel count, so an attempt that
/// misses by a factor divides both sides by the square root of that factor. Raw
/// pixels are exactly linear, so they land in one attempt. A PNG is not,
/// because its size comes off the content as well, so it takes two or three.
///
/// The picture keeps the size that it takes on the screen. The Kitty protocol
/// and the iTerm2 protocol state that size in character cells, beside the
/// payload, so a smaller pixel count loses resolution alone. The Sixel
/// protocol carries no such key, so a Sixel picture gets smaller.
///
/// # Arguments
/// * `image` - The picture at the size the display bounds gave it.
/// * `budget` - The characters of payload that the picture can spend.
/// * `shape` - The shape that the picture starts in, which is the first rung of
///   the ladder that the protocol states.
///
/// # Returns
/// The picture that the payload came off, the shape it ended in, and that
/// payload. The picture comes back untouched and in the shape it started in
/// when that already fits, so a draw inside the budget costs no resize and no
/// second encoder run at all.
///
/// A caller that states the shape in the command reads the shape that comes
/// back here and not the one it passed in, because the fit can step off it.
///
/// A picture that cannot reach the budget comes back at the smallest size the
/// fit could reach, with the payload that size made. Drawing nothing is the
/// one outcome that helps nobody, and the terminal still answers for what it
/// refused.
///
/// # Errors
/// Gives the error of the first encoder run that fails.
fn fit_to_payload_budget<'a, P: Payload>(
    image: Cow<'a, DynamicImage>,
    budget: PayloadBudget,
    shape: P,
) -> Result<(Cow<'a, DynamicImage>, P, String), DrawError> {
    let mut picture = image;
    let mut shape = shape;
    let mut payload = shape.encode(&picture)?;
    let mut resizes = 0;

    while !budget.holds(payload.len()) {
        if let Some(cheaper) = shape.cheaper() {
            shape = cheaper;
            payload = shape.encode(&picture)?;
            continue;
        }

        if resizes == MAXIMUM_FIT_ATTEMPTS {
            break;
        }

        let Some(smaller) = shrink_towards(&picture, budget, payload.len()) else {
            break;
        };

        picture = Cow::Owned(smaller);
        resizes += 1;
        payload = shape.encode(&picture)?;
    }

    Ok((picture, shape, payload))
}

/// Give `image` at the size that aims at `budget`, or [`None`] when no smaller
/// size is left to try.
///
/// # Arguments
/// * `image` - The picture that spent too much.
/// * `budget` - The characters of payload that the picture can spend.
/// * `spent` - The characters that the picture spent.
///
/// # Returns
/// [`None`] when the arithmetic asks for a size that is not smaller than the
/// one it got, which is what a picture at one pixel by one gives. That answer
/// ends the loop, so a budget that no size reaches cannot spin.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a scale in (0.0, 1.0] of an image dimension stays non-negative and inside u32"
)]
fn shrink_towards(
    image: &DynamicImage,
    budget: PayloadBudget,
    spent: usize,
) -> Option<DynamicImage> {
    let scale = ((budget.characters() as f64 / spent as f64) * FIT_SAFETY).sqrt();
    let width = ((f64::from(image.width()) * scale).round() as u32).max(1);
    let height = ((f64::from(image.height()) * scale).round() as u32).max(1);

    if width >= image.width() && height >= image.height() {
        return None;
    }

    Some(image.resize(width, height, FilterType::Lanczos3))
}

/// Write an image with the Kitty graphics protocol.
///
/// The command is `ESC _ G <key>=<value>,... ; <base64 data> ESC \`. A large
/// image goes out in more than one command, because the protocol limits the
/// size of one command. `m=1` says that more data follows and `m=0` closes the
/// image.
///
/// The writer holds the cursor still with [`KITTY_HOLD_CURSOR`] and then states
/// the position of the cursor itself through
/// [`write_image_with_cursor_contract`]. A renderer that also moved the cursor
/// would double the movement. `request.cursor` names which position the writer
/// then states, and it names nothing else about the command.
///
/// The header states which answer the writer wants from the terminal, and
/// `request.picture` names it. [`Picture::Still`] asks for the failures with
/// [`KITTY_FAILURES_ONLY`] and carries an [`ImageNumber`] of its own behind
/// [`KITTY_IMAGE_NUMBER_KEY`], because a terminal answers no transmission that
/// names neither an image id nor an image number, and because that number is
/// what tells the answer of this picture from the answer of another one.
/// [`Picture::Frame`] asks for nothing with [`KITTY_QUIET`], and it names its
/// placement id instead. A Kitty terminal reads the keys of a chunked image
/// from the first chunk alone, and the first chunk is the header, so the keys
/// cover the chunked path as well.
///
/// # The two shapes of the payload
///
/// An image leaves here in one of the two shapes of [`KittyPayload`], and
/// `request.picture` names which one.
///
/// [`Picture::Still`] is one still picture, and it travels as a PNG.
/// Raw pixels cost four base64 characters for every pixel, so a photograph of
/// 330 pixels by 440 costs 580800 characters that way. That is over half of the
/// 1048576 characters that a mosh session holds, and a photograph of twice the
/// pixels never arrives at all. A still picture goes out one time, so the
/// characters are the whole of what it pays, and a PNG of it costs a fraction
/// of the raw pixels.
///
/// [`Picture::Frame`] is one frame of many, and it keeps the raw pixels. The
/// caller draws the next frame directly after this one, so a PNG encoder here
/// runs one time for every frame, and that time costs more than the characters
/// that it saves.
///
/// Ghostty and WezTerm read this same protocol.
///
/// # Arguments
/// * `out` - The stream that takes the bytes.
/// * `image` - The image to draw.
/// * `request` - The budget, the cursor and the aspect ratio.
///
/// # Errors
/// Gives the error of the first write to `out` that fails.
fn write_kitty<W: Write>(
    out: &mut W,
    image: &DynamicImage,
    request: &Request,
    answered: Option<CellPixels>,
) -> Result<(), DrawError> {
    // The window arrives one time, and every size of this image comes off it.
    // Two reads can name two terminals, and a picture laid out for one terminal
    // and reserved for another fits neither.
    let window = termsize::drawing_window();
    let cell = cell_pixels_or_estimate_of(window, answered);
    let (cell_width_px, cell_height_px) = (cell.width(), cell.height());

    // The display size is in terminal cells, and it serves two roles: the `c=`
    // and `r=` keys that tell the terminal how many cells the image spans, and
    // the target of the downscale that caps the pixels this writer sends.
    let (display_width, display_height) = calculate_aspect_preserving_size(
        image.width(),
        image.height(),
        request.budget.columns,
        request.budget.rows,
        request.preserve_aspect,
        cell_aspect_of(cell_width_px, cell_height_px),
    );

    // The downscale keeps the payload off the terminal: a panorama of 16384
    // pixels by 8192 is about 384 megabytes of raw pixels before base64.
    let image = downscale_to_display_pixels(
        image,
        display_width,
        display_height,
        cell_width_px,
        cell_height_px,
    );

    // The picture names the two callers apart. A caller that draws one frame of
    // many draws the next one directly after this one. A caller that draws one
    // still picture pays for it one time, and the characters are the whole of
    // what it pays.
    let shape = match request.picture {
        Picture::Frame { .. } => KittyPayload::RawRgb,
        Picture::Still => KittyPayload::Png,
    };

    // The downscale above bounds the picture by the screen. This bounds it by
    // the characters that the transport carries, which is a second bound and
    // not the same one: mosh caps one transmission at one mebicharacter, and a
    // window of more than about 51 columns by 23 makes a frame above that cap.
    // `c=` and `r=` below still state the cell span that the screen gave, so
    // the picture keeps its size there and loses resolution alone.
    let (image, shape, base64_data) = fit_to_payload_budget(image, request.payload, shape)?;

    let (_, term_rows) = cells_of(window);
    let contract = cursor_contract(request, term_rows, || {
        image_rows_in_cells(
            image.width(),
            image.height(),
            display_width,
            display_height,
            cell_width_px,
            cell_height_px,
        )
    });

    // The two callers want two different answers from the terminal. The caller
    // of a still picture reads the answer and tells the user why the screen is
    // empty. The caller of a frame holds the terminal in raw mode for the key
    // presses of the user, and an answer would arrive there as a key press.
    let answer_keys = match request.picture {
        Picture::Frame { .. } => KITTY_QUIET,
        Picture::Still => KITTY_FAILURES_ONLY,
    };

    // A fixed image id and a fixed placement id make each frame replace the one
    // before it in place, which holds the memory of the renderer flat. A still
    // picture names an image number instead: a terminal answers no transmission
    // that names neither, and a second picture that re-used one image id would
    // delete the first picture. The number is a new one for each still picture,
    // so the answer of this picture carries a name that no other picture wears.
    let image_keys = match request.picture {
        Picture::Frame { id } => format!(",i={id},p={id}"),
        Picture::Still => format!(",{KITTY_IMAGE_NUMBER_KEY}={}", number_of_a_still_picture()),
    };
    let width_key = display_width.map_or_else(String::new, |columns| format!(",c={columns}"));
    let height_key = display_height.map_or_else(String::new, |rows| format!(",r={rows}"));
    let size_keys = shape.pixel_size_keys(image.width(), image.height());
    let header = format!(
        "\x1b_Ga=T,f={},{answer_keys}{size_keys}{image_keys},{KITTY_HOLD_CURSOR}{width_key}{height_key}",
        shape.format_key()
    );

    write_image_with_cursor_contract(out, contract, |sink| {
        if base64_data.len() <= KITTY_CHUNK_SIZE {
            // A small image goes out in one command.
            return write!(sink, "{header};{base64_data}\x1b\\");
        }

        // Base64 holds one ASCII character in one byte, so a chunk of the bytes
        // is always a chunk of the characters.
        let chunks: Vec<&str> = base64_data
            .as_bytes()
            .chunks(KITTY_CHUNK_SIZE)
            .map(|chunk| std::str::from_utf8(chunk).expect("base64 holds ASCII alone"))
            .collect();

        for (index, chunk) in chunks.iter().enumerate() {
            if index == 0 {
                write!(sink, "{header},m=1;{chunk}\x1b\\")?;
            } else if index == chunks.len() - 1 {
                write!(sink, "\x1b_Gm=0;{chunk}\x1b\\")?;
            } else {
                write!(sink, "\x1b_Gm=1;{chunk}\x1b\\")?;
            }
        }

        Ok(())
    })?;

    Ok(())
}

/// Write an image with the Sixel protocol.
///
/// The payload is a palette and then a band of pixels at a time, and an encoder
/// makes it. The protocol takes a size in pixels and not in character cells, so
/// this writer turns the budget of the caller into pixels first.
///
/// A pane of Zellij and a muxiavelli panel both read this protocol.
///
/// # Arguments
/// * `out` - The stream that takes the bytes.
/// * `image` - The image to draw.
/// * `request` - The budget, the cursor and the aspect ratio.
///
/// # Errors
/// Gives [`DrawError::Encode`] when the encoder refuses the image, and the
/// error of the first write to `out` that fails.
fn write_sixel<W: Write>(
    out: &mut W,
    image: &DynamicImage,
    request: &Request,
    answered: Option<CellPixels>,
) -> Result<(), DrawError> {
    // The window arrives one time, and both bounds of this image come off it.
    // The margin takes the pixel size of the window and the budget of the caller
    // takes the size of one cell, so two reads can bound one image by two
    // terminals. The cell size comes from the terminal when it reports a pixel
    // size, and from the estimates when it does not.
    let window = termsize::drawing_window();
    let cell = cell_pixels_or_estimate_of(window, answered);
    let (cell_width_px, cell_height_px) = (cell.width(), cell.height());

    let (target_pixel_width, target_pixel_height) = sixel_pixel_budget(
        window_pixels(window),
        request.budget.columns,
        request.budget.rows,
        cell_width_px,
        cell_height_px,
    );

    let (final_width, final_height) = calculate_sixel_dimensions(
        image.width(),
        image.height(),
        target_pixel_width,
        target_pixel_height,
        request.preserve_aspect,
    );

    // The encoder takes the pixels at the size they draw at, so the image goes
    // to that exact size, up or down.
    let resized = Cow::Owned(image.resize_exact(final_width, final_height, FilterType::Lanczos3));

    // Sixel states its size in pixels and carries no key for a cell span, so a
    // picture that spends fewer pixels is smaller on the screen as well. That
    // is the whole of what the protocol allows, and a smaller picture beats the
    // empty screen that a refused transmission leaves.
    let (resized, _shape, payload) =
        fit_to_payload_budget(resized, request.payload, SixelPayload)?;

    let (_, term_rows) = cells_of(window);
    let contract = cursor_contract(request, term_rows, || {
        image_rows(resized.height(), cell_height_px)
    });

    write_image_with_cursor_contract(out, contract, |sink| write!(sink, "{payload}"))?;

    Ok(())
}

/// Write an image with the iTerm2 inline image protocol.
///
/// The command is `ESC ] 1337 ; File = <arguments> : <base64 data> BEL`. The
/// arguments carry the width and the height in character cells, so they take no
/// `px` suffix.
///
/// The image travels as a whole file, and this writer makes a PNG file of it.
/// The alpha channel goes no further, so the pixels reach the encoder as RGB
/// and not as RGBA.
///
/// The writer holds the cursor still with `doNotMoveCursor=1` and then states
/// the position of the cursor itself through
/// [`write_image_with_cursor_contract`].
///
/// # Arguments
/// * `out` - The stream that takes the bytes.
/// * `image` - The image to draw.
/// * `request` - The budget, the cursor and the aspect ratio.
///
/// # Errors
/// Gives the error of the first write to `out` that fails.
fn write_iterm2<W: Write>(
    out: &mut W,
    image: &DynamicImage,
    request: &Request,
    answered: Option<CellPixels>,
) -> Result<(), DrawError> {
    // The window arrives one time, and every size of this image comes off it.
    // Two reads can name two terminals, and a picture laid out for one terminal
    // and reserved for another fits neither.
    let window = termsize::drawing_window();
    let cell = cell_pixels_or_estimate_of(window, answered);
    let (cell_width_px, cell_height_px) = (cell.width(), cell.height());

    // The display size is in terminal cells, and it serves as both the size
    // arguments of the protocol and the target of the downscale.
    let (display_width, display_height) = calculate_aspect_preserving_size(
        image.width(),
        image.height(),
        request.budget.columns,
        request.budget.rows,
        request.preserve_aspect,
        cell_aspect_of(cell_width_px, cell_height_px),
    );

    let image = downscale_to_display_pixels(
        image,
        display_width,
        display_height,
        cell_width_px,
        cell_height_px,
    );

    // `width=` and `height=` below state the cell span, so a picture that spends
    // fewer pixels keeps the size it takes on the screen. The fit starts at the
    // lossless shape and steps down the qualities of [`Iterm2Payload`] before
    // it takes a pixel off the picture, and the terminal reads the format out
    // of the file, so no argument of the command names the shape it ended in.
    let (image, _shape, base64_data) =
        fit_to_payload_budget(image, request.payload, Iterm2Payload::Png)?;

    let (_, term_rows) = cells_of(window);
    let contract = cursor_contract(request, term_rows, || {
        image_rows_in_cells(
            image.width(),
            image.height(),
            display_width,
            display_height,
            cell_width_px,
            cell_height_px,
        )
    });

    let width_argument =
        display_width.map_or_else(String::new, |columns| format!(";width={columns}"));
    let height_argument = display_height.map_or_else(String::new, |rows| format!(";height={rows}"));

    write_image_with_cursor_contract(out, contract, |sink| {
        write!(
            sink,
            "\x1b]1337;File=inline=1{width_argument}{height_argument};preserveAspectRatio=1;doNotMoveCursor=1:{base64_data}\x07"
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::TerminalType;

    /// The image that the tests draw. One pixel is enough, because no test here
    /// reads the pixels of the payload.
    fn test_image() -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::new(1, 1))
    }

    /// The request that the tests draw with. It states a budget, so no test
    /// depends on the size of the terminal that runs the test.
    fn test_request() -> Request {
        Request {
            budget: Budget {
                columns: Some(10),
                rows: Some(5),
            },
            payload: PayloadBudget::UNLIMITED,
            picture: Picture::Still,
            cursor: Cursor::BelowImage,
            preserve_aspect: true,
        }
    }

    /// The placement id that the frame tests draw with.
    const TEST_PLACEMENT_ID: u32 = 1;

    /// The width in pixels of the photograph that the cost test measures.
    const PHOTOGRAPH_WIDTH: u32 = 330;

    /// The height in pixels of the photograph that the cost test measures.
    const PHOTOGRAPH_HEIGHT: u32 = 440;

    /// Give one colour channel of the photograph fixture as a byte.
    ///
    /// The fixture computes its channels in `u32`, and every value that it
    /// makes stands under 256. The conversion states that instead of assuming
    /// it, so a change of the arithmetic fails the test instead of wrapping in
    /// silence.
    fn channel(value: u32) -> u8 {
        u8::try_from(value).expect("every channel of the fixture stands under 256")
    }

    /// A picture of 330 pixels by 440 that resembles a photograph.
    ///
    /// The report of this defect measures a photograph of exactly this size, so
    /// the fixture holds that size and the test pins the numbers of the report.
    ///
    /// The channels come off smooth functions of the position with a small
    /// repeatable grain on top, because a photograph holds smooth areas and a
    /// little noise. Pure noise is the wrong fixture: a PNG of noise is larger
    /// than the raw pixels it came from, so a test built on it would measure the
    /// one input that this change cannot help.
    fn photograph_fixture() -> DynamicImage {
        photograph_of(PHOTOGRAPH_WIDTH, PHOTOGRAPH_HEIGHT)
    }

    /// A picture of `width` pixels by `height` that resembles a photograph.
    ///
    /// # Arguments
    /// * `width` - The width in pixels, which must be above one.
    /// * `height` - The height in pixels, which must be above one.
    fn photograph_of(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            let grain = (x * 7 + y * 13) % 5;

            image::Rgb([
                channel(x * 200 / (width - 1) + grain),
                channel(y * 180 / (height - 1) + 40 + grain),
                channel((x + y) * 150 / (width + height - 2) + 60 + grain),
            ])
        }))
    }

    /// The width in pixels of the screenshot fixture.
    const SCREENSHOT_WIDTH: u32 = 480;

    /// The height in pixels of the screenshot fixture.
    const SCREENSHOT_HEIGHT: u32 = 320;

    /// A picture of flat color and sharp edges, which is what a screenshot of
    /// text is.
    ///
    /// This is the picture that PNG carries and JPEG cannot. A JPEG spreads
    /// every sharp edge over the block it stands in, so it spends more
    /// characters on this picture than the lossless file does, at any quality.
    /// The fixture therefore holds a few flat colors and hard borders between
    /// them: a title bar, a page, and rows of glyph blocks on it.
    fn screenshot_fixture() -> DynamicImage {
        screenshot_of(SCREENSHOT_WIDTH, SCREENSHOT_HEIGHT)
    }

    /// A picture of `width` pixels by `height` of flat color and sharp edges.
    ///
    /// # Arguments
    /// * `width` - The width in pixels.
    /// * `height` - The height in pixels.
    fn screenshot_of(width: u32, height: u32) -> DynamicImage {
        /// The height in pixels of one row of text.
        const ROW_HEIGHT: u32 = 16;
        /// The height in pixels of the title bar.
        const TITLE_BAR_HEIGHT: u32 = 24;

        DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, y| {
            if y < TITLE_BAR_HEIGHT {
                return image::Rgb([214, 214, 218]);
            }

            let row = (y - TITLE_BAR_HEIGHT) / ROW_HEIGHT;
            let inside_the_line = (y - TITLE_BAR_HEIGHT) % ROW_HEIGHT >= 3
                && (y - TITLE_BAR_HEIGHT) % ROW_HEIGHT < 13;
            let on_a_glyph = (x / 3 + row * 7) % 5 < 2 && x % 9 < 6;

            if inside_the_line && on_a_glyph {
                image::Rgb([28, 28, 36])
            } else {
                image::Rgb([250, 250, 246])
            }
        }))
    }

    /// The characters that mosh holds for one transmission.
    ///
    /// `MAXIMUM_STORED_CHARACTERS` of `crates/mosh-terminal/src/imagestore.rs`,
    /// which `ImageStore::hold` tests against `control.len() + payload.len()`.
    /// A transmission above it earns `ENOSPC` and draws nothing. The file
    /// stands in `timmattison/mosh-rs` at commit `5676142`
    /// (<https://github.com/timmattison/mosh-rs>).
    const MOSH_STORE_CHARACTERS: usize = 1024 * 1024;

    /// The side of the picture that the mosh budget test fits.
    ///
    /// Raw pixels cost four characters each, so this picture costs 1440000 and
    /// stands well above [`PayloadBudget::MOSH`]. A picture under the budget
    /// would leave the fit unrun and the test measuring nothing.
    const OVER_BUDGET_SIDE: u32 = 600;

    /// The characters of payload that the budget tests allow.
    ///
    /// It stands far under the payload of the fixture on any terminal that
    /// runs this suite, so the fit has to do real work whatever cell the
    /// window reports. It also stands far above the floor that
    /// [`fit_to_payload_budget`] stops at, so a fit that ends at the floor
    /// cannot pass the test by accident.
    const TEST_PAYLOAD_BUDGET: usize = 4096;

    /// The share of the payload of an unfitted picture that the Sixel budget
    /// test allows.
    ///
    /// [`TEST_PAYLOAD_BUDGET`] does not serve the Sixel path, for two reasons.
    /// The Sixel writer resizes the picture to the window before the fit runs,
    /// so the payload that the fit starts from moves with the cell size that
    /// the window of the runner reports. A Sixel payload also falls far slower
    /// than the pixel count, so [`fit_to_payload_budget`] needs more than
    /// [`MAXIMUM_FIT_ATTEMPTS`] attempts to reach a budget far under the
    /// payload it starts from.
    ///
    /// A share holds against both. It states a budget under the payload of the
    /// picture at every cell size, so the fit always runs, and it keeps that
    /// budget near enough for the fit to reach it. A measurement on 2026-09-07
    /// swept the pictures that a cell of 6 pixels through a cell of 48 makes. A
    /// share of 90 reaches the budget on every one of them. A share of 85
    /// misses it at a cell of 7 pixels.
    const SIXEL_PAYLOAD_BUDGET_SHARE: usize = 90;

    /// The characters of payload that one Kitty command carries.
    ///
    /// This is the count that mosh keeps: it joins every chunk of one
    /// transmission and holds the whole of it. So the test measures the payload
    /// rather than the bytes of the command, and the chunk headers and the keys
    /// stay out of the number.
    ///
    /// # Arguments
    /// * `command` - The bytes that a writer wrote, as text.
    fn kitty_payload_characters(command: &str) -> usize {
        command
            .split("\x1b_G")
            .skip(1)
            .filter_map(|block| block.split_once(';'))
            .map(|(_keys, rest)| rest.split("\x1b\\").next().unwrap_or_default().len())
            .sum()
    }

    /// Draw `image` on a Kitty terminal inside `budget` and give back the
    /// characters of payload that reached the stream.
    ///
    /// The cursor is [`Cursor::Held`], which writes nothing around the command,
    /// so the count does not move with the window of whoever runs the suite.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `picture` - Whether the picture travels as one still or as one frame.
    /// * `budget` - The characters of payload that the picture can spend.
    fn kitty_payload_of(image: &DynamicImage, picture: Picture, budget: PayloadBudget) -> usize {
        let request = Request {
            payload: budget,
            picture,
            cursor: Cursor::Held,
            ..test_request()
        };

        let mut out = Vec::new();
        Capabilities::new(TerminalType::Kitty, true, true)
            .draw(&mut out, image, &request)
            .expect("a write to a vector never fails");

        kitty_payload_characters(&String::from_utf8(out).expect("a Kitty command is ASCII"))
    }

    /// Draw `image` on a Kitty terminal inside `budget` and give back the keys
    /// of the opening command, which is the part between `ESC _ G` and the
    /// first semicolon.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `picture` - Whether the picture travels as one still or as one frame.
    /// * `budget` - The characters of payload that the picture can spend.
    fn kitty_keys_of(image: &DynamicImage, picture: Picture, budget: PayloadBudget) -> String {
        let request = Request {
            payload: budget,
            picture,
            cursor: Cursor::Held,
            ..test_request()
        };

        let mut out = Vec::new();
        Capabilities::new(TerminalType::Kitty, true, true)
            .draw(&mut out, image, &request)
            .expect("a write to a vector never fails");

        let command = String::from_utf8(out).expect("a Kitty command is ASCII");
        let (keys, _payload) = command
            .split_once(';')
            .expect("a Kitty command holds a semicolon between the keys and the payload");

        String::from(keys)
    }

    /// Draw `image` on a Sixel terminal inside `budget` and give back the
    /// characters of payload that reached the stream.
    ///
    /// The Sixel writer writes the device control string of the encoder and
    /// nothing else, so the bytes of the stream are the payload.
    ///
    /// The cursor is [`Cursor::Held`], which writes nothing around the command,
    /// so the count does not move with the window of whoever runs the suite.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn sixel_payload_of(image: &DynamicImage, budget: PayloadBudget) -> usize {
        let request = Request {
            payload: budget,
            cursor: Cursor::Held,
            ..test_request()
        };

        let mut out = Vec::new();
        Capabilities::new(TerminalType::Zellij, true, true)
            .draw(&mut out, image, &request)
            .expect("a write to a vector never fails");

        out.len()
    }

    /// Draw `image` on an iTerm2 terminal inside `budget` and give back the
    /// command that reached the stream.
    ///
    /// The command is `ESC ] 1337 ; File = <arguments> : <base64 data> BEL`.
    ///
    /// The cursor is [`Cursor::Held`], which writes nothing around the command,
    /// so the command stands alone whatever the window of the runner reports.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn iterm2_command_of(image: &DynamicImage, budget: PayloadBudget) -> String {
        let request = Request {
            payload: budget,
            cursor: Cursor::Held,
            ..test_request()
        };

        let mut out = Vec::new();
        Capabilities::new(TerminalType::ITerm2, true, true)
            .draw(&mut out, image, &request)
            .expect("a write to a vector never fails");

        String::from_utf8(out).expect("an iTerm2 command is ASCII")
    }

    /// Draw `image` on an iTerm2 terminal inside `budget` and give back the
    /// characters of payload that reached the stream.
    ///
    /// The payload is the base64 run between the last colon of the command and
    /// the BEL that closes it. Base64 carries no colon, so the last colon is
    /// the one that opens the payload. The arguments stay out of the number,
    /// as they do for a Kitty command in [`kitty_payload_characters`].
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn iterm2_payload_of(image: &DynamicImage, budget: PayloadBudget) -> usize {
        let command = iterm2_command_of(image, budget);
        let (_arguments, payload) = command
            .rsplit_once(':')
            .expect("an iTerm2 command holds a colon between the arguments and the payload");

        payload.trim_end_matches('\x07').len()
    }

    /// Draw `image` on an iTerm2 terminal inside `budget` and give back the
    /// arguments of the command, which is the part between `File=` and the
    /// last colon.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn iterm2_arguments_of(image: &DynamicImage, budget: PayloadBudget) -> String {
        let command = iterm2_command_of(image, budget);
        let (_introducer, arguments_and_payload) = command
            .split_once("File=")
            .expect("an iTerm2 command holds `File=` before its arguments");
        let (arguments, _payload) = arguments_and_payload
            .rsplit_once(':')
            .expect("an iTerm2 command holds a colon between the arguments and the payload");

        String::from(arguments)
    }

    /// The first bytes of a PNG file, which name the format to a reader.
    ///
    /// The iTerm2 protocol carries a whole file, and the terminal reads the
    /// format out of the first bytes of it. So a test that asks which format a
    /// picture travelled in reads those same bytes.
    const PNG_SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    /// The first bytes of a JPEG file, which are the start-of-image marker.
    const JPEG_SIGNATURE: &[u8] = &[0xff, 0xd8];

    /// The request that the encoder tests draw with.
    ///
    /// It states no bound in character cells, so the picture reaches the
    /// encoder at its own pixel size. A bound in cells makes the writer
    /// downscale the picture to the window of whoever runs the suite, and a
    /// test that reads the file would then read a file of a size that the
    /// window decided.
    ///
    /// # Arguments
    /// * `budget` - The characters of payload that the picture can spend.
    fn whole_picture_request(budget: PayloadBudget) -> Request {
        Request {
            budget: Budget {
                columns: None,
                rows: None,
            },
            payload: budget,
            picture: Picture::Still,
            cursor: Cursor::Held,
            preserve_aspect: true,
        }
    }

    /// Draw `image` on an iTerm2 terminal at its own pixel size, inside
    /// `budget`, and give back the base64 payload of the command.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn iterm2_whole_picture_payload_of(image: &DynamicImage, budget: PayloadBudget) -> String {
        let mut out = Vec::new();
        Capabilities::new(TerminalType::ITerm2, true, true)
            .draw(&mut out, image, &whole_picture_request(budget))
            .expect("a write to a vector never fails");

        let command = String::from_utf8(out).expect("an iTerm2 command is ASCII");
        let (_arguments, payload) = command
            .rsplit_once(':')
            .expect("an iTerm2 command holds a colon between the arguments and the payload");

        String::from(payload.trim_end_matches('\x07'))
    }

    /// Draw `image` on an iTerm2 terminal at its own pixel size, inside
    /// `budget`, and give back the file that the command carried.
    ///
    /// # Arguments
    /// * `image` - The picture to draw.
    /// * `budget` - The characters of payload that the picture can spend.
    fn iterm2_file_of(image: &DynamicImage, budget: PayloadBudget) -> Vec<u8> {
        BASE64_STANDARD
            .decode(iterm2_whole_picture_payload_of(image, budget))
            .expect("the writer wrote base64")
    }

    /// The Kitty graphics command that takes every image off the screen. The
    /// test spells the bytes out, so a change of the command fails the test
    /// instead of moving with it.
    const KITTY_DELETE_ALL_BYTES: &str = "\x1b_Ga=d,d=A\x1b\\";

    /// Clear the images of one terminal and give back the bytes.
    fn cleared(terminal_type: TerminalType) -> String {
        let mut out = Vec::new();
        Capabilities::new(terminal_type, true, true)
            .clear_images(&mut out)
            .expect("a write to a vector never fails");

        String::from_utf8(out).expect("the delete command is ASCII")
    }

    /// Draw one frame of many on a Kitty terminal and give back the control
    /// data of the command, which is the part between `ESC _ G` and the
    /// semicolon.
    ///
    /// The cursor is [`Cursor::Held`], which takes the caller managed cursor
    /// contract. That contract reserves no rows, so the command is the same in
    /// every terminal that runs the suite. [`Cursor::BelowImage`] bounds its
    /// reservation by the height of the window that the writer measures, so no
    /// test here draws with it.
    fn kitty_control_data() -> String {
        let request = Request {
            picture: Picture::Frame {
                id: TEST_PLACEMENT_ID,
            },
            cursor: Cursor::Held,
            ..test_request()
        };

        let mut out = Vec::new();
        Capabilities::new(TerminalType::Kitty, true, true)
            .draw(&mut out, &test_image(), &request)
            .expect("a write to a vector never fails");

        let command = String::from_utf8(out).expect("a Kitty command is ASCII");
        let (control_data, _payload) = command
            .split_once(';')
            .expect("a Kitty command holds a semicolon between the keys and the payload");

        String::from(control_data)
    }

    /// Draw one still picture on a Kitty terminal and give back the control
    /// data of the command, which is the part between `ESC _ G` and the first
    /// semicolon after it.
    ///
    /// The cursor is [`Cursor::BelowImage`], which is the contract that a still
    /// picture takes when the caller states no other one. That contract writes
    /// newlines, a CUU and a DECSC before the payload, and the count of the
    /// newlines comes off the window of whoever runs the suite. The control
    /// data stands after all of them and holds none of them, so this slice is
    /// the same in every terminal.
    fn kitty_still_control_data() -> String {
        let mut out = Vec::new();
        Capabilities::new(TerminalType::Kitty, true, true)
            .draw(&mut out, &test_image(), &test_request())
            .expect("a write to a vector never fails");

        let command = String::from_utf8(out).expect("a Kitty command is ASCII");
        let (_reservation, keys_and_payload) = command
            .split_once("\x1b_G")
            .expect("a Kitty command holds the APC introducer and the G before its keys");
        let (control_data, _payload) = keys_and_payload
            .split_once(';')
            .expect("a Kitty command holds a semicolon between the keys and the payload");

        String::from(control_data)
    }

    #[test]
    fn a_still_picture_travels_as_a_png() {
        // Raw pixels cost four base64 characters for every pixel, and a mosh
        // session holds 1048576 characters of image, so one photograph takes
        // over half of that store and a photograph of twice the pixels never
        // arrives. `f=100` names a PNG instead, and a Kitty terminal then reads
        // the width and the height out of the PNG itself. The header must carry
        // no `s=` key and no `v=` key beside it.
        let control_data = kitty_still_control_data();

        assert!(
            control_data.contains("f=100"),
            "a still picture must travel as a PNG, but the keys are {control_data:?}"
        );
        assert!(
            !control_data.contains(",s="),
            "a PNG states its own width, so the keys must hold no s= key, but they are {control_data:?}"
        );
        assert!(
            !control_data.contains(",v="),
            "a PNG states its own height, so the keys must hold no v= key, but they are {control_data:?}"
        );
    }

    #[test]
    fn a_video_frame_keeps_the_raw_pixels() {
        // A PNG encoder runs one time for every frame of a video, and that time
        // costs more than the characters that it saves. A frame therefore keeps
        // `f=24`. Raw pixels state no size of their own, so the `s=` key and the
        // `v=` key must stay beside them.
        let control_data = kitty_control_data();

        assert!(
            control_data.contains("f=24"),
            "a video frame must keep the raw pixels, but the keys are {control_data:?}"
        );
        assert!(
            control_data.contains(",s=1"),
            "raw pixels state no width, so the keys must state it, but they are {control_data:?}"
        );
        assert!(
            control_data.contains(",v=1"),
            "raw pixels state no height, so the keys must state it, but they are {control_data:?}"
        );
    }

    /// A frame above the budget comes back inside it.
    ///
    /// A frame keeps the raw pixels, so its payload is exactly four characters
    /// for every pixel. mosh caps one transmission at one mebicharacter, which
    /// is 262144 pixels, or a window of about 51 columns by 23. Every larger
    /// window drew no frame at all.
    #[test]
    fn a_frame_above_the_payload_budget_comes_back_inside_it() {
        let spent = kitty_payload_of(
            &photograph_fixture(),
            Picture::Frame {
                id: TEST_PLACEMENT_ID,
            },
            PayloadBudget::of(TEST_PAYLOAD_BUDGET),
        );

        assert!(
            spent <= TEST_PAYLOAD_BUDGET,
            "a frame must spend at most {TEST_PAYLOAD_BUDGET} characters, but it spent {spent}"
        );
        assert!(
            spent > 0,
            "a frame that spends nothing drew nothing, which is the failure this repairs"
        );
    }

    /// A still picture above the budget comes back inside it.
    ///
    /// A still travels as a PNG, whose size comes off the content and not off
    /// the pixel count alone, so the fit measures the encoder rather than
    /// solving for a size.
    #[test]
    fn a_still_picture_above_the_payload_budget_comes_back_inside_it() {
        let spent = kitty_payload_of(
            &photograph_fixture(),
            Picture::Still,
            PayloadBudget::of(TEST_PAYLOAD_BUDGET),
        );

        assert!(
            spent <= TEST_PAYLOAD_BUDGET,
            "a still must spend at most {TEST_PAYLOAD_BUDGET} characters, but it spent {spent}"
        );
        assert!(
            spent > 0,
            "a still that spends nothing drew nothing, which is the failure this repairs"
        );
    }

    /// A Sixel picture above the budget comes back inside it.
    ///
    /// The green commit that made the fit wired it into all three writers, and
    /// this test holds the Sixel one. A Sixel picture states its size in pixels
    /// and carries no key for a cell span, so the fit takes room off the
    /// picture as well as resolution. That is the whole of what the protocol
    /// allows, and a smaller picture beats the empty screen that a refused
    /// transmission leaves.
    ///
    /// [`SIXEL_PAYLOAD_BUDGET_SHARE`] says why the budget is a share of the
    /// payload of the picture and not a count of characters.
    #[test]
    fn a_sixel_picture_above_the_payload_budget_comes_back_inside_it() {
        let fixture = photograph_fixture();
        let whole = sixel_payload_of(&fixture, PayloadBudget::UNLIMITED);
        let budget = whole * SIXEL_PAYLOAD_BUDGET_SHARE / 100;
        let spent = sixel_payload_of(&fixture, PayloadBudget::of(budget));

        assert!(
            budget < whole,
            "the budget must stand under the payload of the picture, or the fit never runs and this test measures nothing"
        );
        assert!(
            spent <= budget,
            "a Sixel picture must spend at most {budget} characters, but it spent {spent}"
        );
        assert!(
            spent > 0,
            "a Sixel picture that spends nothing drew nothing, which is the failure this repairs"
        );
    }

    /// A picture that the budget holds travels as a PNG file.
    ///
    /// The iTerm2 protocol carries a whole file, and a raw PNM file spends
    /// three bytes on every pixel and compresses none of them. A photograph of
    /// 3074 pixels by 1856 costs 17116032 bytes that way, which is 22821376
    /// base64 characters, and a mosh session holds 1048576 of them. The same
    /// photograph as a PNG costs a fraction of it and loses no pixel at all.
    #[test]
    fn a_picture_that_the_budget_holds_travels_as_a_png() {
        let file = iterm2_file_of(&photograph_fixture(), PayloadBudget::UNLIMITED);

        assert!(
            file.starts_with(PNG_SIGNATURE),
            "an iTerm2 picture that the budget holds must travel as a PNG, but the file starts with {:?}",
            &file[..PNG_SIGNATURE.len().min(file.len())]
        );
    }

    /// The pixel size of the file that an iTerm2 command carried.
    ///
    /// # Arguments
    /// * `file` - The bytes that the command carried.
    fn pixels_of(file: &[u8]) -> (u32, u32) {
        let picture = image::load_from_memory(file).expect("the writer wrote a whole image file");

        (picture.width(), picture.height())
    }

    /// A photograph above the budget spends quality before it spends pixels.
    ///
    /// A PNG is lossless, and a photograph compresses poorly in it, so a PNG of
    /// a photograph stands far above what a mosh session holds. The fit used to
    /// answer that with pixels alone: it shrank the picture until the PNG fit,
    /// and the terminal then stretched a small picture over the same cells. A
    /// JPEG carries the same photograph at a fraction of the cost, so the
    /// quality goes down first and every pixel stays.
    #[test]
    fn a_photograph_above_the_budget_spends_quality_before_pixels() {
        let fixture = photograph_fixture();
        let whole = iterm2_whole_picture_payload_of(&fixture, PayloadBudget::UNLIMITED);
        let budget = PayloadBudget::of(whole.len() / 2);
        let file = iterm2_file_of(&fixture, budget);

        assert!(
            file.starts_with(JPEG_SIGNATURE),
            "a photograph that the budget cannot hold as a PNG must travel as a JPEG, but the file starts with {:?}",
            &file[..JPEG_SIGNATURE.len().min(file.len())]
        );
        assert_eq!(
            pixels_of(&file),
            (PHOTOGRAPH_WIDTH, PHOTOGRAPH_HEIGHT),
            "a picture that reached the budget on quality alone must keep every pixel"
        );
    }

    /// A picture of flat color above the budget keeps its lossless file.
    ///
    /// A JPEG spreads every sharp edge over the block it stands in, so a
    /// screenshot of text costs more as a JPEG than as a PNG at every quality
    /// of the ladder. A fit that stepped down that ladder anyway would spend
    /// the quality of the picture and reach no budget with it, and it would
    /// then spend the pixels as well. So the fit reads what the encoder makes
    /// of this picture and keeps the shape that costs the fewest characters.
    #[test]
    fn a_picture_of_flat_color_above_the_budget_keeps_its_png() {
        let fixture = screenshot_fixture();
        let whole = iterm2_whole_picture_payload_of(&fixture, PayloadBudget::UNLIMITED);
        let budget = PayloadBudget::of(whole.len() / 2);
        let file = iterm2_file_of(&fixture, budget);

        assert!(
            file.starts_with(PNG_SIGNATURE),
            "a picture that costs more as a JPEG must keep its PNG, but the file starts with {:?}",
            &file[..PNG_SIGNATURE.len().min(file.len())]
        );
        assert!(
            pixels_of(&file).0 < SCREENSHOT_WIDTH,
            "the budget must be one that the picture cannot reach at its own size, or this test measures nothing"
        );
    }

    /// An iTerm2 picture above the budget comes back inside it.
    ///
    /// The green commit that made the fit wired it into all three writers, and
    /// this test holds the iTerm2 one.
    #[test]
    fn an_iterm2_picture_above_the_payload_budget_comes_back_inside_it() {
        let spent = iterm2_payload_of(
            &photograph_fixture(),
            PayloadBudget::of(TEST_PAYLOAD_BUDGET),
        );

        assert!(
            spent <= TEST_PAYLOAD_BUDGET,
            "an iTerm2 picture must spend at most {TEST_PAYLOAD_BUDGET} characters, but it spent {spent}"
        );
        assert!(
            spent > 0,
            "an iTerm2 picture that spends nothing drew nothing, which is the failure this repairs"
        );
    }

    /// A picture fitted for mosh fits the store that mosh keeps, keys and all.
    ///
    /// [`PayloadBudget::MOSH`] bounds the payload alone, and mosh counts the
    /// control block with it. So the room that the budget leaves has to be real
    /// room, measured against the keys of a real command, and not a number that
    /// looks generous.
    ///
    /// Two assertions carry that, and they catch two different mistakes.
    /// The budget against the store catches a budget with no room in it for
    /// the keys, and it reads the budget rather than the fit, because
    /// [`FIT_SAFETY`] leaves five percent of its own and would hide a thin
    /// allowance. The fitted payload against the store catches a budget raised
    /// above the store itself.
    ///
    /// Both mutations were measured on 2026-09-07. A `MOSH` of one
    /// mebicharacter fails the first, and a `MOSH` of two fails the second.
    /// Every other test here states a budget of its own and passes with either
    /// mistake in place.
    #[test]
    fn a_picture_fitted_for_mosh_fits_the_store_that_mosh_keeps() {
        let picture = photograph_of(OVER_BUDGET_SIDE, OVER_BUDGET_SIDE);
        let whole = KittyPayload::RawRgb
            .encode(&picture)
            .expect("raw pixels reach base64 with no encoder that can refuse them");

        let (_fitted, _shape, payload) = fit_to_payload_budget(
            Cow::Borrowed(&picture),
            PayloadBudget::MOSH,
            KittyPayload::RawRgb,
        )
        .expect("raw pixels reach base64 with no encoder that can refuse them");

        assert!(
            whole.len() > MOSH_STORE_CHARACTERS,
            "the fixture must stand above the store, or the fit never runs and this test measures nothing"
        );
        assert!(
            payload.len() < whole.len(),
            "the fit must really take pixels off a picture that stands above the budget"
        );

        let keys = kitty_keys_of(
            &picture,
            Picture::Frame {
                id: TEST_PLACEMENT_ID,
            },
            PayloadBudget::MOSH,
        );
        // The budget itself has to leave room for the keys, whatever the fit
        // does with it. A payload that spends the whole budget is the payload
        // that a picture just above it produces.
        let allowed = PayloadBudget::MOSH.characters() + keys.len();
        assert!(
            allowed <= MOSH_STORE_CHARACTERS,
            "mosh holds the keys and the payload together, and the budget plus the keys of a real command come to {allowed}, which is above {MOSH_STORE_CHARACTERS}"
        );

        let held = payload.len() + keys.len();
        assert!(
            held <= MOSH_STORE_CHARACTERS,
            "mosh holds the keys and the payload together, and the two come to {held}, which is above {MOSH_STORE_CHARACTERS}"
        );
    }

    /// The fit takes resolution off a picture and takes no room off it.
    ///
    /// `c=` and `r=` state how many cells the picture spans, and the terminal
    /// scales the pixels it got into them. So the keys come off the screen
    /// bounds, ahead of the fit, and a picture that spends fewer pixels holds
    /// the size that the user sees.
    ///
    /// A fit that derived those keys from the pixels it ended at would shrink
    /// the picture on the screen instead, which is the mistake this guards.
    #[test]
    fn a_fit_that_shrinks_the_payload_keeps_the_cell_span() {
        let fixture = photograph_fixture();
        let frame = Picture::Frame {
            id: TEST_PLACEMENT_ID,
        };

        let generous = kitty_keys_of(&fixture, frame, PayloadBudget::UNLIMITED);
        let tight = kitty_keys_of(&fixture, frame, PayloadBudget::of(TEST_PAYLOAD_BUDGET));

        let span = |keys: &str| -> Vec<String> {
            keys.split(',')
                .filter(|key| key.starts_with("c=") || key.starts_with("r="))
                .map(String::from)
                .collect()
        };

        assert!(
            !span(&generous).is_empty(),
            "the test reads nothing unless the command states a cell span, but the keys are {generous:?}"
        );
        assert_eq!(
            span(&tight),
            span(&generous),
            "a picture that spent fewer pixels must span the same cells, but the keys went from {generous:?} to {tight:?}"
        );

        // A payload that did not move proves nothing about a span that did not
        // move either, so the test states that the fit really ran.
        assert!(
            kitty_payload_of(&fixture, frame, PayloadBudget::of(TEST_PAYLOAD_BUDGET))
                < kitty_payload_of(&fixture, frame, PayloadBudget::UNLIMITED),
            "the tight budget must really take pixels off the picture"
        );
    }

    /// The fit takes resolution off an iTerm2 picture and takes no room off it.
    ///
    /// `width=` and `height=` state how many cells the picture spans, and the
    /// terminal scales the pixels it got into them. So the arguments come off
    /// the screen bounds, ahead of the fit, and a picture that spends fewer
    /// pixels holds the size that the user sees.
    ///
    /// A fit that takes those arguments off the pixels it ended at shrinks the
    /// picture on the screen instead, which is the mistake this guards. It is
    /// the same mistake that
    /// [`a_fit_that_shrinks_the_payload_keeps_the_cell_span`] guards for the
    /// Kitty writer.
    #[test]
    fn an_iterm2_fit_that_shrinks_the_payload_keeps_the_cell_span() {
        let fixture = photograph_fixture();

        let generous = iterm2_arguments_of(&fixture, PayloadBudget::UNLIMITED);
        let tight = iterm2_arguments_of(&fixture, PayloadBudget::of(TEST_PAYLOAD_BUDGET));

        let span = |arguments: &str| -> Vec<String> {
            arguments
                .split(';')
                .filter(|argument| {
                    argument.starts_with("width=") || argument.starts_with("height=")
                })
                .map(String::from)
                .collect()
        };

        assert!(
            !span(&generous).is_empty(),
            "the test reads nothing unless the command states a cell span, but the arguments are {generous:?}"
        );
        assert_eq!(
            span(&tight),
            span(&generous),
            "a picture that spent fewer pixels must span the same cells, but the arguments went from {generous:?} to {tight:?}"
        );

        // A payload that did not move proves nothing about a span that did not
        // move either, so the test states that the fit really ran.
        assert!(
            iterm2_payload_of(&fixture, PayloadBudget::of(TEST_PAYLOAD_BUDGET))
                < iterm2_payload_of(&fixture, PayloadBudget::UNLIMITED),
            "the tight budget must really take pixels off the picture"
        );
    }

    /// A picture under the budget is left alone.
    ///
    /// The fit must not cost a picture that already fits one pixel of
    /// resolution, and `UNLIMITED` must reach the encoder unchanged.
    #[test]
    fn a_picture_under_the_payload_budget_keeps_every_pixel() {
        let fixture = photograph_fixture();
        let generous = kitty_payload_of(
            &fixture,
            Picture::Frame {
                id: TEST_PLACEMENT_ID,
            },
            PayloadBudget::UNLIMITED,
        );
        let ample = kitty_payload_of(
            &fixture,
            Picture::Frame {
                id: TEST_PLACEMENT_ID,
            },
            PayloadBudget::of(generous),
        );

        assert_eq!(
            ample, generous,
            "a budget that the picture already fits must take no pixel off it"
        );
    }

    #[test]
    fn a_still_picture_costs_less_than_four_characters_for_every_pixel() {
        // This test reads the encoder and not `draw`, on purpose. `draw`
        // measures the window of whoever runs the suite and resizes the picture
        // to it, so a cost measured through `draw` is a cost measured on one
        // terminal. The encoder takes the picture that the caller gives it, so
        // this measurement is the same everywhere.
        let fixture = photograph_fixture();

        let raw = KittyPayload::RawRgb
            .encode(&fixture)
            .expect("raw pixels reach base64 with no encoder that can refuse them");
        let png = KittyPayload::Png
            .encode(&fixture)
            .expect("the PNG encoder takes an RGB8 picture of this size");

        // Base64 turns three bytes into four characters, and three bytes is one
        // pixel. The pixel count of this picture divides by three, so the
        // payload takes no padding and the count is exact. It is the number that
        // the report of the defect names.
        let raw_cost = usize::try_from(4 * PHOTOGRAPH_WIDTH * PHOTOGRAPH_HEIGHT)
            .expect("the cost of one small photograph fits in a machine word");
        assert_eq!(
            raw.len(),
            raw_cost,
            "raw pixels cost exactly four base64 characters for every pixel"
        );

        let budget = raw.len() * 60 / 100;
        assert!(
            png.len() <= budget,
            "a PNG of this photograph must cost at most {budget} characters, which is 60 percent of the raw pixels, but it costs {}",
            png.len()
        );

        // A payload that merely got smaller proves nothing. `iVBORw0KGgo` is the
        // base64 of the signature that every PNG file starts with.
        assert!(
            png.starts_with("iVBORw0KGgo"),
            "the payload must be a PNG file, but it starts with {:?}",
            png.chars().take(11).collect::<String>()
        );
    }

    #[test]
    fn a_video_frame_asks_the_terminal_for_no_answer() {
        // A Kitty terminal answers an image command that names an image id, and
        // the answer is an APC sequence on the terminal itself. A tool that
        // holds that terminal in raw mode reads the answer as key presses, so
        // the image of one row of a frame arrives at the tool as a command of
        // the user. `q=2` takes both answers of the terminal away, and an image
        // number would ask for an answer that nothing reads.
        let control_data = kitty_control_data();

        assert!(
            control_data.contains("q=2"),
            "the keys of a video frame must hold q=2, but they are {control_data:?}"
        );
        assert!(
            !control_data.contains("I="),
            "a video frame asks for no answer, so the keys must name no image number, but they are {control_data:?}"
        );
    }

    #[test]
    fn a_still_picture_asks_the_terminal_for_the_failures_alone() {
        // A terminal that refuses a still picture draws nothing, and a tool
        // that asked for no answer then reports success in front of an empty
        // screen. `q=1` takes the success answer away and leaves the failure
        // answer, which the caller of the writer reads.
        //
        // The answer needs a name to hang on: a Kitty terminal answers a
        // transmission only when the transmission names an image id or an image
        // number. The `I` key carries the image number, and it is an `I` and
        // not an `i` because a re-transmission of an image id deletes the image
        // that held it. A fixed `i` would take the picture of one run off the
        // screen the moment the next run drew.
        let control_data = kitty_still_control_data();

        assert!(
            control_data.contains("q=1"),
            "a still picture must ask for the failures with q=1, but the keys are {control_data:?}"
        );
        assert!(
            !control_data.contains("q=2"),
            "q=2 takes the failure answer away as well, so a still picture must not carry it, but the keys are {control_data:?}"
        );
        let number = still_image_number_of(&control_data).expect(
            "a terminal answers no transmission that names neither an image id nor an image number, so a still picture must carry an I= key",
        );
        assert!(
            number != 0,
            "zero names no image to a Kitty terminal, and the keys are {control_data:?}"
        );
        assert!(
            !control_data.contains(",i="),
            "a re-transmission of an image id deletes the image that held it, so a still picture must name no image id, but the keys are {control_data:?}"
        );
    }

    /// The image number that `control_data` carries, or [`None`] for keys that
    /// carry none.
    ///
    /// The keys are `<name>=<value>` pairs that a comma divides, and this reads
    /// the whole value behind the capital `I`.
    ///
    /// # Arguments
    /// * `control_data` - The keys of one Kitty command.
    ///
    /// # Returns
    /// The number, for keys that name one and give it a value that a 32-bit
    /// number holds.
    fn still_image_number_of(control_data: &str) -> Option<u32> {
        control_data
            .split(',')
            .find_map(|pair| pair.strip_prefix(&format!("{KITTY_IMAGE_NUMBER_KEY}=")))?
            .parse()
            .ok()
    }

    #[test]
    fn two_still_pictures_carry_two_image_numbers() {
        // The image number is the one key that says which picture a terminal
        // speaks about. Two pictures of one number wear one name, so the
        // refusal of the first one names the second one as well, and the reader
        // of that answer reports a failure for a picture that drew.
        let first = still_image_number_of(&kitty_still_control_data())
            .expect("a still picture carries an image number");
        let second = still_image_number_of(&kitty_still_control_data())
            .expect("a still picture carries an image number");

        assert_ne!(
            first, second,
            "two still pictures must carry two image numbers, and both carried {first}"
        );
    }

    #[test]
    fn an_image_number_stands_away_from_the_numbers_of_another_program() {
        // A second program that draws Kitty pictures on the same terminal
        // numbers its own pictures, and a program that counts from one picks
        // small numbers. A refusal of such a program must name no picture of
        // this run, so every number of this crate stands above the floor.
        let number = still_image_number_of(&kitty_still_control_data())
            .expect("a still picture carries an image number");

        assert!(
            number >= IMAGE_NUMBER_FLOOR.get(),
            "an image number must stand above {IMAGE_NUMBER_FLOOR}, where a program that counts its pictures from one never reaches, and this picture carried {number}"
        );
    }

    #[test]
    fn a_kitty_terminal_deletes_the_placements_it_holds() {
        // A Kitty placement outlives a clear of the screen, so a caller that
        // draws one image for each frame stacks the whole run on the screen.
        assert_eq!(cleared(TerminalType::Kitty), KITTY_DELETE_ALL_BYTES);
    }

    #[test]
    fn a_terminal_that_paints_into_the_screen_needs_no_delete_command() {
        // The iTerm2 protocol and the Sixel protocol both paint their pixels
        // into the screen and hold no handle on them. A delete command would
        // therefore say nothing that the terminal can act on.
        assert_eq!(cleared(TerminalType::ITerm2), "");
        assert_eq!(cleared(TerminalType::Zellij), "");
    }

    #[test]
    fn a_terminal_that_draws_no_image_says_so_and_writes_no_byte() {
        // Alacritty draws text alone. A tool that sent it an escape sequence of
        // an image would put the sequence on the screen as text, so the answer
        // has to come back before one byte leaves.
        let capabilities = Capabilities::new(TerminalType::Alacritty, false, true);

        let mut out = Vec::new();
        let error = capabilities
            .draw(&mut out, &test_image(), &test_request())
            .expect_err("a terminal that draws no image must refuse the image");

        assert!(
            matches!(error, DrawError::NoGraphics),
            "the refusal must name the terminal as the reason, but it is {error:?}"
        );
        assert!(
            out.is_empty(),
            "the refusal must leave the stream untouched, but it holds {} bytes",
            out.len()
        );
    }
}
