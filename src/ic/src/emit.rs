//! The output of `--emit-rgba`: the pixels of one picture, as JSON, for a
//! program that draws the picture itself.
//!
//! Some programs own every cell of the screen and draw each cell again when
//! something changes. Claude Code is one. An escape sequence that a child
//! writes past such a program lands at a position that its next frame paints
//! over, so the program draws a picture with its own elements, and it needs
//! the pixels and nothing else. This module gives it those pixels.
//!
//! **Nothing here reads or writes the terminal.** The program that runs the
//! flag owns the terminal and reads the keys of the user from it, so a query
//! from `ic` would put an answer in front of that program, which reads the
//! answer as keys. The caller in `main` therefore detects no terminal for this
//! flag, and this module takes a decoded picture and an output stream alone.
//!
//! # The document
//!
//! One JSON object on one line, then a newline:
//!
//! ```json
//! {"width":10,"height":5,"source_width":40,"source_height":20,"rgba":"..."}
//! ```
//!
//! * `width` and `height` - The size of the printed picture, in pixels. It
//!   fits inside the box of the flag, and it keeps the shape of the source.
//! * `source_width` and `source_height` - The size of the picture in the file.
//! * `rgba` - Four bytes for each pixel (red, green, blue, alpha), row by row
//!   from the top left, in standard base64 with padding.
//!
//! The module writes the object itself and not through `serde_json`, because
//! the pixels of a picture of 4096 by 4096 take 89 megabytes of base64, and a
//! JSON value would hold that whole string in memory before the first byte
//! leaves. Every field is a whole number or base64, and neither one holds a
//! character that JSON escapes, so the stream is valid JSON as it stands.

use anyhow::Result;
use base64::engine::general_purpose::STANDARD;
use base64::write::EncoderWriter;
use image::imageops::FilterType;
use image::DynamicImage;
use std::borrow::Cow;
use std::io::Write;
use std::str::FromStr;

/// The longest side of a box that the flag takes, in pixels.
///
/// The pixels of a box of 4096 by 4096 take 64 megabytes, which is a large
/// picture and still a bounded one. A larger box asks for a copy that no
/// terminal shows.
pub const BOX_LIMIT: u32 = 4096;

/// The box of pixels that the printed picture fits inside.
///
/// Each side holds 1 to [`BOX_LIMIT`] pixels. The only way to make one is to
/// parse it, so a box of no pixels cannot exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBox {
    /// The widest that the picture can be, in pixels.
    width: u32,
    /// The tallest that the picture can be, in pixels.
    height: u32,
}

impl FromStr for PixelBox {
    type Err = String;

    /// Read a box written as `WIDTHxHEIGHT`, such as `640x480`.
    ///
    /// # Errors
    /// A message that states the shape of a box, when `text` is not two whole
    /// numbers from 1 to [`BOX_LIMIT`] joined by a lowercase `x`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let side = |part: &str| {
            part.parse::<u32>()
                .ok()
                .filter(|pixels| (1..=BOX_LIMIT).contains(pixels))
        };
        let shape = || {
            format!("expected two whole numbers from 1 to {BOX_LIMIT} joined by x, such as 640x480")
        };

        let (width, height) = text.split_once('x').ok_or_else(shape)?;
        match (side(width), side(height)) {
            (Some(width), Some(height)) => Ok(PixelBox { width, height }),
            _ => Err(shape()),
        }
    }
}

/// The size that a picture of `width` by `height` pixels takes inside
/// `pixel_box`.
///
/// A picture that fits keeps its size, because a larger copy holds no detail
/// that the file did not hold and the program that draws it scales it itself.
/// A picture that does not fit takes the whole of the side that binds it, and
/// the other side keeps the shape of the picture, rounded to the nearest
/// pixel. That side keeps one pixel at least, because a picture of no pixels
/// is no picture.
///
/// # Arguments
/// * `width` - The width of the source picture, in pixels. It is 1 or more.
/// * `height` - The height of the source picture, in pixels. It is 1 or more.
/// * `pixel_box` - The box that the result fits inside.
///
/// # Returns
/// The width and the height of the fitted picture, each inside the box.
fn fit_inside(width: u32, height: u32, pixel_box: PixelBox) -> (u32, u32) {
    if width <= pixel_box.width && height <= pixel_box.height {
        return (width, height);
    }

    // The products of two sides of at most 2^32 fit in 64 bits, so the
    // comparison and the rounding below are exact.
    let (width, height) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let (box_width, box_height) = (u64::from(pixel_box.width), u64::from(pixel_box.height));

    // The source is at least as wide as the box, in shape, when its width over
    // its height is at least the width of the box over its height. The width
    // then binds.
    let (fitted_width, fitted_height) = if width * box_height >= height * box_width {
        let scaled = (height * box_width + width / 2) / width;
        (box_width, scaled.clamp(1, box_height))
    } else {
        let scaled = (width * box_height + height / 2) / height;
        (scaled.clamp(1, box_width), box_height)
    };

    // Both values are inside the box, whose sides are `u32`, so each
    // conversion succeeds. The fallback is the side of the box, which is the
    // largest value the conversion could have given.
    (
        u32::try_from(fitted_width).unwrap_or(pixel_box.width),
        u32::try_from(fitted_height).unwrap_or(pixel_box.height),
    )
}

/// Write `picture`, fitted inside `pixel_box`, as the JSON document of the
/// flag.
///
/// The module documentation states the document. The base64 streams straight
/// into `out`, so no copy of the whole text stands in memory.
///
/// # Arguments
/// * `picture` - The decoded picture.
/// * `pixel_box` - The box that the printed picture fits inside.
/// * `out` - Where the document goes. This call writes nothing else there.
///
/// # Errors
/// An error when a write to `out` fails.
pub fn write_rgba_json(
    picture: &DynamicImage,
    pixel_box: PixelBox,
    out: &mut impl Write,
) -> Result<()> {
    let (source_width, source_height) = (picture.width(), picture.height());
    let (width, height) = fit_inside(source_width, source_height, pixel_box);

    // A picture that fits needs no resize. The filter is the one that
    // `termgfx` resizes a display with, so the two outputs of `ic` agree.
    let fitted = if (width, height) == (source_width, source_height) {
        Cow::Borrowed(picture)
    } else {
        Cow::Owned(picture.resize_exact(width, height, FilterType::Lanczos3))
    };
    let pixels = fitted.to_rgba8();

    write!(
        out,
        "{{\"width\":{width},\"height\":{height},\"source_width\":{source_width},\"source_height\":{source_height},\"rgba\":\""
    )?;
    {
        // The encoder borrows `out` until it drops, so it drops at the end of
        // this block, before the close of the document.
        let mut encoder = EncoderWriter::new(&mut *out, &STANDARD);
        encoder.write_all(pixels.as_raw())?;
        encoder.finish()?;
    }
    writeln!(out, "\"}}")?;
    out.flush()?;

    Ok(())
}
