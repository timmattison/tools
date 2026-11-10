//! The pull-request indicator of the watch-mode header.
//!
//! This file is the root of the module. Each part of the indicator lives in a
//! submodule of it. [`target`] decides which pull request the indicator asks about.

mod target;

pub(crate) use target::{target_of, PullTarget};
