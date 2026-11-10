#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

//! GitHub REST code that more than one tool needs.
//!
//! Each tool in this workspace that talks to GitHub through `gh` used to keep
//! its own copy of the same few pieces. This crate holds each piece once:
//!
//! - [`GithubRepo`] reads the URL of a git remote and names the repository.
//! - [`PullNumber`] is the number of one pull request.
//! - [`Response`] and [`parse_response`] read the output of `gh api -i`.
//!
//! The crate starts no command and uses no network. Every function reads text
//! that the caller already has, so every test runs with no `gh` and no
//! network.

mod pull;
mod reply;
mod repo;

pub use pull::PullNumber;
pub use reply::{parse_response, Response};
pub use repo::GithubRepo;
