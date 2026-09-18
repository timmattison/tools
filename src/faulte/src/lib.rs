//! `faulte` — "fault rate": rank the processes of this Mac by page faults per
//! second, and stop the old idle Claude Code sessions.
//!
//! A Mac that is short of memory spends its time in the kernel. The kernel
//! moves pages between memory, the compressor, and swap. The process that
//! causes this work is the process that makes page faults now, so the signal is
//! the rate of page faults over an interval. The counter since each process
//! started is not the signal: an old process ranks high on that counter
//! whatever it does now.
//!
//! On 2026-09-18, 213 Claude Code sessions made 90% of all page faults on this
//! Mac, and no single session made much of it. The load was the sum of many
//! small rates, and 86 of those sessions were older than 7 days. Thus `faulte`
//! gives a total line for the Claude Code sessions, and `faulte kill` stops the
//! old idle ones.
//!
//! The rules are pure functions over plain values, so a test can check each one
//! without a real process table.

pub mod duration;
pub mod pid;
pub mod top;
