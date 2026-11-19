//! The one-instance lock and the background start of a tool that runs one
//! copy of itself for each target, such as a menu bar item.
//!
//! - [`instance`] — the lock that marks the instance of a target, and the
//!   calls to the kernel that detach it from the terminal and that stop it.
//! - [`launch`] — what one run does with the instance of its target: start it
//!   here, start it in the background, or stop it. A tool implements
//!   [`launch::Program`] and calls [`launch::run`].
//! - `testing` (feature `testing`) — a helper that holds a lock in a
//!   different process, for tests.
//!
//! Both modules build on every Unix, because the calls are POSIX.
//!
//! # Unsafe code
//!
//! The crate denies `unsafe_code`. Only [`instance`] expects it, so a review
//! of that one file covers all of the unsafe code.

#![deny(unsafe_code)]
#![cfg(unix)]

#[expect(
    unsafe_code,
    reason = "the instance module calls the kernel through libc for its lock, its signals, and its session. It is the one module that holds unsafe code"
)]
pub mod instance;
pub mod launch;
