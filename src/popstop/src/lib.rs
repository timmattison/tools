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

pub mod signal;
