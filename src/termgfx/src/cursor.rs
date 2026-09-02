//! Where the cursor stands after a routine writes an image.
//!
//! Sixel gives no contract for the position of the cursor after the string
//! terminator, and each renderer decides for itself. The Kitty protocol and the
//! iTerm2 protocol each have a flag that holds the cursor still, but then the
//! caller must move it. A tool that draws an image therefore states the
//! position of the cursor instead of a guess, and this module holds the one
//! statement of it.

use std::io::{self, Write};

/// Control sequence introducer. It starts a CSI escape sequence.
const CSI: &str = "\x1b[";

/// DECSC. It saves the position of the cursor.
const SAVE_CURSOR: &str = "\x1b7";

/// DECRC. It restores the saved position of the cursor.
const RESTORE_CURSOR: &str = "\x1b8";

/// Calculate the number of rows that the cursor contract reserves for an image.
///
/// The reservation is one newline for each row of the image, but the height of
/// the terminal bounds it. An image taller than the screen has no row below it,
/// and CUU and CUD both stop at the edge of the screen, so no reservation can
/// put the cursor below such an image. A reservation larger than the screen only
/// scrolls the content of the user out of view and gives nothing back for it.
/// The bound therefore holds the scroll to one screen.
///
/// # Arguments
/// * `image_rows` - The height of the image in terminal rows.
/// * `term_height` - The height of the terminal in rows.
///
/// # Returns
/// The number of rows to reserve. The result leaves one row for the cursor to
/// land on, and it is always 1 or more, so the contract never asks the terminal
/// for a movement of zero rows, which a terminal reads as one row.
fn reservation_rows(image_rows: u32, term_height: u32) -> u32 {
    image_rows.min(term_height.saturating_sub(1)).max(1)
}

/// Where a display routine must leave the cursor after it writes an image.
///
/// Sixel gives no contract for the position of the cursor after the string
/// terminator, and each renderer decides for itself. The Kitty protocol and the
/// iTerm2 protocol each have a flag that holds the cursor still, but then the
/// caller must move it. A tool therefore states the position of the cursor
/// instead of a guess.
pub(crate) enum CursorContract {
    /// The caller puts the cursor where it wants it (`--no-newline`, video
    /// playback). The routine writes the payload and nothing else.
    CallerManaged,
    /// Column 1 of the first row below the image.
    BelowImage {
        /// The number of rows that the contract reserves, which is the height
        /// of the image in terminal rows bounded by the height of the
        /// terminal. [`CursorContract::below_image`] applies the bound.
        rows: u32,
    },
}

impl CursorContract {
    /// Give the contract for one image of a display routine.
    ///
    /// This is the one place that bounds the reservation by the height of the
    /// terminal, so no display routine can reserve more rows than the terminal
    /// has. An image taller than the screen has no row below it, and CUU and
    /// CUD both stop at the edge of the screen, so a reservation larger than
    /// the screen only scrolls the content of the user out of view and gives
    /// nothing back for it. The bound therefore holds the scroll to one screen.
    ///
    /// The height of the terminal arrives from the caller, and this function
    /// reads no terminal itself. The writer of one image measures one window
    /// and hands the rows of that window to every step of that image, so the
    /// picture and the reservation below it name one terminal. A second read
    /// here could name a second one, and a picture laid out for one window and
    /// reserved for another fits neither. That is the mismatch of GitHub issue
    /// #350.
    ///
    /// The row count of the image arrives as a closure, because the caller must
    /// convert the pixels of the image with the size of a character cell to
    /// compute it. Video playback asks for [`CursorContract::CallerManaged`]
    /// one time for each frame, and the closure keeps that arithmetic off that
    /// path.
    ///
    /// # Arguments
    /// * `no_newline` - True when the caller puts the cursor where it wants it.
    /// * `term_rows` - The height of the terminal in rows, off the one window
    ///   that the writer measured.
    /// * `image_rows` - Gives the height of the image in terminal rows.
    ///
    /// # Returns
    /// The promise that the display routine must keep.
    pub(crate) fn below_image(
        no_newline: bool,
        term_rows: u32,
        image_rows: impl FnOnce() -> u32,
    ) -> Self {
        if no_newline {
            return CursorContract::CallerManaged;
        }

        CursorContract::BelowImage {
            rows: reservation_rows(image_rows(), term_rows),
        }
    }
}

/// Write an image payload and keep the promise of a [`CursorContract`].
///
/// Every display routine owes the caller the same promise: the cursor ends at
/// column 1 of the first row below the image, unless the caller asked for no
/// newline. This function holds the one implementation of that promise, so a
/// new display routine cannot forget it.
///
/// [`CursorContract::BelowImage`] writes four parts:
///
/// 1. One newline for each row of the image, bounded by the height of the
///    terminal. The reservation makes an image at the bottom of the screen
///    scroll the terminal instead of run off it. An image taller than the
///    screen cannot have a row below it, so the bound holds the scroll to one
///    screen. [`CursorContract::below_image`] applies the bound.
/// 2. CUU, which goes back to the top of the reservation.
/// 3. DECSC, the payload and DECRC. The brackets make sure that cursor motion
///    inside the payload cannot change the final position of the cursor.
/// 4. CUD by the row count and a carriage return, which together put the cursor
///    at column 1 of the first row below the image.
///
/// The payload arrives as a closure, because the Kitty routine writes its
/// payload in more than one chunk and cannot make one string.
///
/// # Arguments
/// * `out` - The stream that takes the bytes.
/// * `contract` - The promise that the routine must keep.
/// * `payload` - A closure that writes the image payload to `out`.
///
/// # Errors
/// Gives the error of the first write to `out` that fails, which includes a
/// failure inside the payload closure.
pub(crate) fn write_image_with_cursor_contract<W, F>(
    out: &mut W,
    contract: CursorContract,
    payload: F,
) -> io::Result<()>
where
    W: Write,
    F: FnOnce(&mut W) -> io::Result<()>,
{
    let CursorContract::BelowImage { rows } = contract else {
        return payload(out);
    };

    // The row count is always 1 or more, so this never asks the terminal for a
    // movement of zero rows.
    for _ in 0..rows {
        writeln!(out)?;
    }
    write!(out, "{CSI}{rows}A")?;

    write!(out, "{SAVE_CURSOR}")?;
    payload(out)?;
    write!(out, "{RESTORE_CURSOR}")?;

    write!(out, "{CSI}{rows}B\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Tests for reservation_rows
    // =========================================================================

    #[test]
    fn reservation_rows_takes_the_whole_image_when_it_fits() {
        // An image that leaves room for the row below it keeps every row.
        assert_eq!(reservation_rows(5, 24), 5);
        assert_eq!(reservation_rows(23, 24), 23);
    }

    #[test]
    fn reservation_rows_stops_at_the_height_of_the_terminal() {
        // An image taller than the screen has no row below it, so the
        // reservation keeps the scroll to one screen.
        assert_eq!(reservation_rows(39, 24), 23);
        assert_eq!(reservation_rows(100, 24), 23);
        assert_eq!(reservation_rows(24, 24), 23);
    }

    #[test]
    fn reservation_rows_never_gives_zero() {
        // A movement of zero rows means one row to a terminal, so the
        // reservation must stay at 1 or more, even in a terminal of no height.
        assert_eq!(reservation_rows(1, 1), 1);
        assert_eq!(reservation_rows(5, 0), 1);
        assert_eq!(reservation_rows(0, 24), 1);
    }

    // =========================================================================
    // Tests for CursorContract::below_image
    // =========================================================================

    /// The rows that one contract reserves, or `None` when the caller states
    /// the position of the cursor itself.
    ///
    /// [`CursorContract`] carries no `PartialEq`, so a test names the variant
    /// through this function instead of comparing two contracts.
    fn reserved_rows(contract: CursorContract) -> Option<u32> {
        match contract {
            CursorContract::CallerManaged => None,
            CursorContract::BelowImage { rows } => Some(rows),
        }
    }

    #[test]
    fn below_image_bounds_the_reservation_by_the_rows_it_is_given() {
        // The writer measures one window and hands the rows of that window
        // over, so the bound must come off the number in the argument and off
        // no other terminal. The two cases below state two different terminals
        // for one image of 50 rows, and no one terminal answers both: a
        // reservation that read a terminal of its own would give the same
        // bound to both calls.
        assert_eq!(
            reserved_rows(CursorContract::below_image(false, 7, || 50)),
            Some(6),
            "an image of 50 rows in a terminal of 7 leaves the last row for the cursor to land on"
        );
        assert_eq!(
            reserved_rows(CursorContract::below_image(false, 100, || 50)),
            Some(50),
            "an image of 50 rows fits inside a terminal of 100, so the reservation keeps every row of it"
        );
        assert_eq!(
            reserved_rows(CursorContract::below_image(true, 7, || 50)),
            None,
            "the caller that asks for no newline states the position of the cursor itself"
        );
    }

    // =========================================================================
    // Tests for write_image_with_cursor_contract
    // =========================================================================

    /// The payload that the contract tests write, which stands for the escape
    /// sequence of a real image.
    const PAYLOAD: &str = "<image>";

    /// Write `PAYLOAD` under one contract and give back the bytes.
    fn written(contract: CursorContract) -> String {
        let mut out = Vec::new();
        write_image_with_cursor_contract(&mut out, contract, |sink| write!(sink, "{PAYLOAD}"))
            .expect("a write to a vector never fails");
        String::from_utf8(out).expect("the contract writes ASCII around an ASCII payload")
    }

    #[test]
    fn a_caller_managed_contract_writes_the_payload_and_nothing_else() {
        // The caller states the position of the cursor itself, so the contract
        // must not move it.
        assert_eq!(written(CursorContract::CallerManaged), PAYLOAD);
    }

    #[test]
    fn a_below_image_contract_reserves_the_rows_and_lands_below_them() {
        // Three newlines make room for a three row image, CUU goes back to the
        // top of that room, DECSC and DECRC bracket the payload, and CUD with a
        // carriage return lands at column 1 of the row below the image.
        assert_eq!(
            written(CursorContract::BelowImage { rows: 3 }),
            format!("\n\n\n{CSI}3A{SAVE_CURSOR}{PAYLOAD}{RESTORE_CURSOR}{CSI}3B\r")
        );
    }
}
