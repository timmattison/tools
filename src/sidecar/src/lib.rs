#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
//! sidecar — make, clone, and link the private data repositories of a host repository.
//!
//! A host repository names its sidecars in `sidecar.toml`. This library holds the
//! rules that the `sidecar` binary uses.

pub mod git;
pub mod link;
pub mod name;
pub mod registry;
pub mod remote;
pub mod setup;
