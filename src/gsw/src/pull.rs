//! The pull-request indicator of the watch-mode header.
//!
//! This file is the root of the module. Each part of the indicator lives in a
//! submodule of it. [`target`] decides which pull request the indicator asks about.
//! [`indicator`] holds the states of the indicator and their text.
//! [`lookup`] asks GitHub for the open pull requests and reads the reply.
//! [`settings`] holds the `gh` program and the host that the lookups use.
//! [`state`] decides when a lookup starts and what the indicator shows.

mod indicator;
mod lookup;
mod settings;
mod state;
mod target;

pub(crate) use indicator::{from_numbers, PullHeader, PullIndicator};
pub(crate) use lookup::{run_lookup, LookupOutcome, LOOKUP_LIMIT};
pub(crate) use settings::{PullSettings, GH_HOST_VARIABLE, PATH_VARIABLE};
pub(crate) use state::{LookupRequest, PullWatch};
pub(crate) use target::{target_of, PullTarget};
