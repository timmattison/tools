//! The full-screen view of `faulte tui`.
//!
//! - [`app`] — the state of the view. One pure value takes a key or an update
//!   and changes. It holds no terminal and no clock.
//! - [`view`] — the paint of one screen. One function reads the state and
//!   draws it. It changes nothing but the page size of the state.

pub mod app;
pub mod view;
