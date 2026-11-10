//! The pull-request indicator of the watch-mode header.
//!
//! This file is the root of the module. Each part of the indicator lives in a
//! submodule of it. [`target`] decides which pull request the indicator asks about.
//! [`indicator`] holds the states of the indicator and their text.

mod indicator;
mod target;

pub(crate) use indicator::{from_numbers, PullHeader, PullIndicator};
pub(crate) use target::{target_of, PullTarget};
