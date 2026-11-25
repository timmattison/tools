//! The one place that names the cargo that a guard starts.
//!
//! Each guard that runs cargo asks this module for the program. Thus all
//! guards use the same rule and the rules cannot drift apart.

use std::env;
use std::ffi::OsString;

/// The variable cargo sets to its own path for the processes it starts.
const CARGO_ENV: &str = "CARGO";

/// What to start when nothing names a cargo.
const CARGO_FALLBACK: &str = "cargo";

/// The cargo to start: the one that started this process, when there is one.
///
/// Read it before a caller scrubs the child environment. Then a run under
/// `cargo test` uses the same toolchain as the test run itself.
pub(crate) fn program() -> OsString {
    env::var_os(CARGO_ENV).unwrap_or_else(|| OsString::from(CARGO_FALLBACK))
}
