//! The one reader of the terminal that draws an image, and the sizes that an
//! image needs before it draws.
//!
//! A tool that puts a real image on a terminal has three questions to answer
//! before it writes one byte, and none of the three has an obvious answer.
//!
//! * Which terminal is this, and does it draw an image at all. Three escape
//!   sequences are in service and no terminal reads all three.
//!   [`Capabilities::detect`] names the terminal from the environment
//!   variables that the terminal set. A terminal that set none of them
//!   answers two of the three questions itself, and
//!   [`Capabilities::detect_by_asking`] asks it. See the `probe` module. A
//!   tool that reports what the terminal is and draws nothing takes
//!   [`Capabilities::detect_by_asking_the_protocol`], which asks about the
//!   protocol alone and spends no round trip on the size of a cell it never
//!   reads.
//! * How many pixels does one character cell hold. A terminal lays text out in
//!   cells and it draws an image in pixels, so a tool that wants an image of a
//!   given number of cells has to convert. [`cell_pixels`] measures one cell,
//!   and it gives `None` for a terminal that reports no pixel size.
//! * Where does the cursor stand after the image. No image protocol promises a
//!   position, and each renderer decides for itself, so [`Cursor`] states the
//!   position instead of guessing it.
//!
//! A caller answers none of the three itself. It reads the terminal once with
//! [`Capabilities::detect`], builds a [`Request`], and calls
//! [`Capabilities::draw`]. The choice of protocol, the base64 and the escape
//! sequences all stay inside this crate.
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
//! `detect` reads the environment and names the terminal. `probe` asks a
//! terminal of no name and a pane of a multiplexer, and it reads the refusal
//! that a terminal wrote for a picture it would not draw. `geometry` measures
//! a character cell and every size that comes off it. `cursor` states where the
//! cursor ends. `draw` holds the three writers, one for each protocol.
//! `session` reads what the environment of a mosh session states about the
//! images it carries, which is the one answer no query can reach from inside a
//! multiplexer. Each module keeps its tests beside the code they cover.
//!
//! Only a small part of the six modules leaves the crate. The routing, the
//! arithmetic of the sizes and the cursor contract are all steps of one call,
//! and a caller that reached them one at a time would hold the parts of a
//! picture that only this crate knows how to put together. The list below is
//! therefore short on purpose, and it grows only when a caller has a use for
//! an answer that [`Capabilities::draw`] cannot give it.
//!
//! [`travels_as_it_stands`] is one such answer. A caller states the file a
//! picture came out of in [`Request::source`], and the draw sends that file
//! byte for byte in two formats alone, and out of those two only for a file
//! that holds one picture and states no turn of it. The draw reads the rule
//! too late for the caller, which holds the whole file from the moment it
//! reads it, so a caller that asks first drops the bytes of a file that no
//! draw can send and holds the picture by itself.

mod cursor;
mod detect;
mod draw;
mod geometry;
mod probe;
mod session;

pub use detect::{AnsweredProtocol, Capabilities, ImageProtocol, TerminalType};
pub use draw::{travels_as_it_stands, Budget, Cursor, DrawError, PayloadBudget, Picture, Request};
pub use geometry::{cell_pixels, terminal_cells};
pub use probe::Refusal;
pub use session::{MoshImages, ProtocolSet};
