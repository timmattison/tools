//! `popstop` — pop stop.
//!
//! macOS stops the audio stream to an output device a few seconds after the
//! last sound. The next sound starts the stream again. On USB speakers, that
//! restart loses the start of the sound and makes a pop. `popstop` plays a
//! signal that is too small to hear into the default output device, so the
//! stream never stops.
//!
//! This library holds the parts of the tool that the tests reach:
//!
//! - [`signal`] makes the samples of the keepalive signal. It is pure and it
//!   compiles on every platform.
//! - [`lock`] makes sure that only one copy runs for each user. It uses the
//!   standard library only and it compiles on every platform.
//! - `output` plays the signal through the default output unit of Core Audio.
//!   It compiles on macOS only.
//! - `process` reads the kernel start time of a process. It compiles on macOS
//!   only.

pub mod lock;
#[cfg(target_os = "macos")]
pub mod output;
#[cfg(target_os = "macos")]
pub mod process;
pub mod signal;
