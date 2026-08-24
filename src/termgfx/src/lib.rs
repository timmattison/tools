//! The one reader of the terminal that draws an image, and the sizes that an
//! image needs before it draws.
//!
//! A tool that puts a real image on a terminal has three questions to answer
//! before it writes one byte, and none of the three has an obvious answer.
//!
//! * Which terminal is this, and does it draw an image at all. Three escape
//!   sequences are in service, no terminal reads all three, and a terminal
//!   answers no question about the ones it reads. [`Capabilities::detect`]
//!   names the terminal from the environment variables that the terminal set,
//!   and [`display_routine_for`] turns that name into the one routine that
//!   draws.
//! * How many pixels does one character cell hold. A terminal lays text out in
//!   cells and it draws an image in pixels, so a tool that wants an image of a
//!   given number of cells has to convert. [`cell_pixels`] measures one cell,
//!   and it gives `None` for a terminal that reports no pixel size.
//! * Where does the cursor stand after the image. No image protocol promises a
//!   position, and each renderer decides for itself. [`CursorContract`] states
//!   the position instead of guessing it, and
//!   [`write_image_with_cursor_contract`] keeps that promise for every routine
//!   that writes an image.
//!
//! # Why the answers stand in one crate
//!
//! `ic` wrote all of this first, and it wrote it in its own binary crate. A
//! binary crate builds a program, and it builds no library, so no other tool
//! of this workspace reads one line of it. `krt` draws the history of a hop
//! with block characters, where a real image says more in the same columns,
//! and `krt` therefore needs the same answers. The only way to reach them was
//! to write them a second time.
//!
//! Two detectors are worse than the one they replace. Each of them reads the
//! environment on its own, so the two of them disagree the day one of them
//! learns a new terminal, and a user then sees an image in one tool and blocks
//! in the other on the same screen. The rules are also the kind that a second
//! author gets wrong: the order the environment variables are read in carries
//! the whole of the correctness, and nothing in the environment says so. The
//! same holds for the sizes. A cell of ten pixels by twenty is a guess, and a
//! tool that guesses it on its own draws an image of a size that no other tool
//! of the workspace agrees with.
//!
//! # The modules
//!
//! `detect` reads the environment and names the terminal. `geometry` measures
//! a character cell and every size that comes off it. `cursor` states where the
//! cursor ends. Each module keeps its tests beside the code they cover, and
//! this file re-exports the whole surface that a caller reads.

mod cursor;
mod detect;
mod geometry;

pub use cursor::{write_image_with_cursor_contract, CursorContract};
pub use detect::{display_routine_for, Capabilities, DisplayRoutine, ImageProtocol, TerminalType};
pub use geometry::{
    calculate_aspect_preserving_size, calculate_sixel_dimensions, cell_aspect_ratio, cell_pixels,
    cell_pixels_or_estimate, downscale_to_display_pixels, image_rows, image_rows_in_cells,
    sixel_pixel_budget, terminal_cells, terminal_pixels,
};
