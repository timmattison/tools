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
//! - `background` starts a copy that has no terminal, and runs the life cycle
//!   of such a copy. It compiles on macOS only.
//! - `control` holds the commands that act on the copy that runs, for
//!   `--status` and `--stop`. It compiles on macOS only.
//! - [`exit_status`] names the exit statuses and lists them for `--help`.
//! - [`handshake`] holds the report that a background copy sends to the
//!   parent that started it.
//! - [`message`] builds the texts that popstop shows to the user.
//! - `keepalive` plays the signal on the default output device. It compiles
//!   on macOS only.
//! - `life_cycle` runs a copy from its start to its stop. It compiles on
//!   macOS only.
//! - [`signal`] makes the samples of the keepalive signal. It is pure and it
//!   compiles on every platform.
//! - [`lock`] makes sure that only one copy runs for each user. It uses the
//!   standard library only and it compiles on every platform.
//! - `output` plays the signal through the default output unit of Core Audio.
//!   It compiles on macOS only.
//! - `process` reads the kernel start time of a process. It compiles on macOS
//!   only.

#[cfg(target_os = "macos")]
pub mod background;
#[cfg(target_os = "macos")]
pub mod control;
pub mod exit_status;
pub mod handshake;
#[cfg(target_os = "macos")]
pub mod keepalive;
#[cfg(target_os = "macos")]
pub mod life_cycle;
pub mod lock;
pub mod message;
#[cfg(target_os = "macos")]
pub mod output;
#[cfg(target_os = "macos")]
pub mod process;
pub mod signal;
