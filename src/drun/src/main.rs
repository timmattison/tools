//! `drun` runs a Docker container on the least busy of your Docker hosts, and
//! finds the container again later.
//!
//! A host is a Docker context. `drun run` asks every host how busy it is,
//! chooses the least busy host, and hands the arguments to
//! `docker --context <host> run`. The other commands find the host that has a
//! container, and hand their arguments to the docker command of that host.

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]

mod config;
mod docker_args;
mod fleet;
mod host;
mod survey;
mod table;

use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::FAILURE
}
