//! Core logic for `prgz` (Progress Gzip): gzip-compress one file and report
//! what the run cost.
//!
//! The binary owns the progress bar and the command line. This library owns the
//! two parts that a test can drive without a terminal: the compression itself,
//! and the closing report.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]
