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
//! image in a different shape. Kitty takes the raw pixels in base64, in chunks
//! of a fixed size, under a list of keys. iTerm2 takes a whole image file in
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

use std::io::{self, Write};

use base64::prelude::{Engine, BASE64_STANDARD};
use icy_sixel::{sixel_encode, EncodeOptions};
use image::imageops::FilterType;
use image::DynamicImage;

use crate::cursor::{write_image_with_cursor_contract, CursorContract};
use crate::detect::{display_routine_for, Capabilities, DisplayRoutine};
use crate::geometry::{
    calculate_aspect_preserving_size, calculate_sixel_dimensions, cell_aspect_of,
    cell_pixels_or_estimate_of, cells_of, downscale_to_display_pixels, image_rows,
    image_rows_in_cells, sixel_pixel_budget, window_pixels,
};

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
/// answer is an APC sequence on the terminal itself. This crate reads no answer
/// of any protocol, so every answer is waste at best. It is worse than waste
/// for a caller that holds the terminal in raw mode: the answer arrives at that
/// caller as key presses, and `krt` then reads the `p` of `i=1,p=1;OK` as the
/// pause command of its own live table. `q=2` takes the success answer and the
/// failure answer both away.
///
/// [`KITTY_DELETE_ALL`] carries no such key, because it names no image id and a
/// Kitty terminal answers it never.
const KITTY_QUIET: &str = "q=2";

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

/// Where the cursor stands when the image is written.
///
/// No image protocol promises a position of the cursor, and each renderer
/// decides for itself, so the caller states the position it wants instead of a
/// guess.
///
/// The placement `id` of [`Cursor::Held`] means something to the Kitty
/// graphics protocol alone. The Sixel protocol and the iTerm2 protocol paint
/// into the screen and hold no handle on what they painted, so the two writers
/// of those protocols read the id and then ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    /// The crate writes the image and moves the cursor to the row under it.
    BelowImage,
    /// The caller holds the cursor, and the crate moves nothing. The image
    /// carries the placement `id`, so a later image of that same id replaces
    /// it in place instead of standing beside it.
    Held {
        /// The placement id of the image, which a Kitty terminal reads and the
        /// other two protocols ignore.
        id: u32,
    },
}

/// One image, and what the caller asks the terminal to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// How much of the terminal the image can take.
    pub budget: Budget,
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

        match display_routine_for(self.terminal_type()) {
            DisplayRoutine::Sixel => write_sixel(out, image, request),
            DisplayRoutine::Kitty => write_kitty(out, image, request),
            DisplayRoutine::Iterm2 => write_iterm2(out, image, request),
        }?;

        out.flush()?;
        Ok(())
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
        if display_routine_for(self.terminal_type()) == DisplayRoutine::Kitty {
            write!(out, "{KITTY_DELETE_ALL}")?;
        }

        Ok(())
    }
}

/// Give the cursor contract that one request asks for.
///
/// [`Cursor::Held`] is the caller-managed contract, because the caller that
/// names a placement id is the caller that puts the cursor where it wants it.
///
/// # Arguments
/// * `request` - The request that names the cursor.
/// * `image_rows` - Gives the height of the image in terminal rows. It runs
///   only for [`Cursor::BelowImage`], and [`CursorContract::below_image`] reads
///   the height of the terminal only there as well. A video frame asks for
///   [`Cursor::Held`] one time for each frame, so neither the arithmetic nor
///   that read stands on its path.
///
/// # Returns
/// The promise that the writer must keep.
fn cursor_contract(
    request: &Request,
    term_rows: u32,
    image_rows: impl FnOnce() -> u32,
) -> CursorContract {
    CursorContract::below_image(
        matches!(request.cursor, Cursor::Held { .. }),
        term_rows,
        image_rows,
    )
}

/// Write an image with the Kitty graphics protocol.
///
/// The command is `ESC _ G <key>=<value>,... ; <base64 data> ESC \`. A large
/// image goes out in more than one command, because the protocol limits the
/// size of one command. `m=1` says that more data follows and `m=0` closes the
/// image.
///
/// The writer holds the cursor still with `C=1` and then states the position of
/// the cursor itself through [`write_image_with_cursor_contract`]. A renderer
/// that also moved the cursor would double the movement.
///
/// The header carries [`KITTY_QUIET`], so the terminal answers no command of
/// this writer. The key stands in the header and not beside the keys of one
/// cursor mode, because every image of every mode wants the silence. A Kitty
/// terminal reads the keys of a chunked image from the first chunk alone, and
/// the first chunk is the header, so the key covers the chunked path as well.
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
) -> Result<(), DrawError> {
    // The window arrives one time, and every size of this image comes off it.
    // Two reads can name two terminals, and a picture laid out for one terminal
    // and reserved for another fits neither.
    let window = termsize::drawing_window();
    let (cell_width_px, cell_height_px) = cell_pixels_or_estimate_of(window);

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

    // The protocol takes the raw pixels, so this path needs no image encoder.
    let rgb = image.to_rgb8();
    let base64_data = BASE64_STANDARD.encode(rgb.as_raw());

    let contract = cursor_contract(request, cells_of(window).1, || {
        image_rows_in_cells(
            image.width(),
            image.height(),
            display_width,
            display_height,
            cell_width_px,
            cell_height_px,
        )
    });

    // A fixed image id and a fixed placement id make each frame of a video
    // replace the one before it in place, which holds the memory of the
    // renderer flat.
    let cursor_keys = match request.cursor {
        Cursor::Held { id } => format!(",i={id},p={id},C=1"),
        Cursor::BelowImage => String::from(",C=1"),
    };
    let width_key = display_width.map_or_else(String::new, |columns| format!(",c={columns}"));
    let height_key = display_height.map_or_else(String::new, |rows| format!(",r={rows}"));
    let header = format!(
        "\x1b_Ga=T,f=24,{KITTY_QUIET},s={},v={}{cursor_keys}{width_key}{height_key}",
        image.width(),
        image.height()
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
) -> Result<(), DrawError> {
    // The window arrives one time, and both bounds of this image come off it.
    // The margin takes the pixel size of the window and the budget of the caller
    // takes the size of one cell, so two reads can bound one image by two
    // terminals. The cell size comes from the terminal when it reports a pixel
    // size, and from the estimates when it does not.
    let window = termsize::drawing_window();
    let (cell_width_px, cell_height_px) = cell_pixels_or_estimate_of(window);

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
    let resized = image.resize_exact(final_width, final_height, FilterType::Lanczos3);
    let rgba = resized.to_rgba8();

    let payload = sixel_encode(
        rgba.as_raw(),
        resized.width() as usize,
        resized.height() as usize,
        &EncodeOptions::default(),
    )
    .map_err(|error| DrawError::Encode(error.to_string()))?;

    let contract = cursor_contract(request, cells_of(window).1, || {
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
/// The image travels as a whole file, and this writer makes a PNM file by hand:
/// a header of three lines and then the raw pixels. Three bytes for one pixel
/// is a smaller file than four, so the pixels go out as RGB and not as RGBA.
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
) -> Result<(), DrawError> {
    // The window arrives one time, and every size of this image comes off it.
    // Two reads can name two terminals, and a picture laid out for one terminal
    // and reserved for another fits neither.
    let window = termsize::drawing_window();
    let (cell_width_px, cell_height_px) = cell_pixels_or_estimate_of(window);

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

    let rgb = image.to_rgb8();
    let rgb_data = rgb.as_raw();
    let pnm_header = format!("P6\n{} {}\n255\n", image.width(), image.height());
    let mut pnm_data = Vec::with_capacity(pnm_header.len() + rgb_data.len());
    pnm_data.extend_from_slice(pnm_header.as_bytes());
    pnm_data.extend_from_slice(rgb_data);
    let base64_data = BASE64_STANDARD.encode(&pnm_data);

    let contract = cursor_contract(request, cells_of(window).1, || {
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
            cursor: Cursor::BelowImage,
            preserve_aspect: true,
        }
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

    /// Draw one image on a Kitty terminal and give back the control data of
    /// the command, which is the part between `ESC _ G` and the semicolon.
    ///
    /// The cursor is [`Cursor::Held`], which takes the caller managed cursor
    /// contract. That contract reads nothing off the terminal, so the command
    /// is the same in every terminal that runs the suite. [`Cursor::BelowImage`]
    /// reads the height of the terminal, so no test here draws with it.
    fn kitty_control_data() -> String {
        let request = Request {
            cursor: Cursor::Held { id: 1 },
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

    #[test]
    fn a_kitty_image_asks_the_terminal_for_no_answer() {
        // A Kitty terminal answers an image command that names an image id, and
        // the answer is an APC sequence on the terminal itself. A tool that
        // holds that terminal in raw mode reads the answer as key presses, so
        // the image of one row of a frame arrives at the tool as a command of
        // the user. `q=2` takes both answers of the terminal away.
        let control_data = kitty_control_data();

        assert!(
            control_data.contains("q=2"),
            "the keys of a Kitty image must hold q=2, but they are {control_data:?}"
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
