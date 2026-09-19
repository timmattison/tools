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
//! On 2026-09-18, 213 Claude Code processes made 90% of all page faults on this
//! Mac, and no single one of them made much of it. The load was the sum of many
//! small rates, and 86 of those sessions were older than 7 days. Thus `faulte`
//! gives a total line for the Claude Code sessions, and `faulte kill` stops the
//! old idle ones.
//!
//! `faulte` reads this Mac through `/usr/bin/top` and `/bin/ps`. Both of them
//! carry the entitlement `com.apple.system-task-ports.read`, so both read the
//! processes of every account. A third-party binary cannot get that
//! entitlement, and it reads almost nothing about a process of another account.
//!
//! The rules are pure functions over plain values, so a test can check each one
//! without a real process table.
//!
//! # The modules
//!
//! - [`duration`] — a length of time on a command line, such as `5s` or `7d`.
//! - [`pid`] — the numbers that name a process and an account.
//! - [`top`] — the command line of the fault sampler, and the parser of its
//!   output. It reads the second sample, which holds the rate.
//! - [`table`] — the command line of the process table, and the parser of its
//!   output.
//! - [`vm`] — the counters of the virtual memory system, and the swap traffic
//!   over the sample.
//! - [`state`] — the state of a Claude Code session, from its registry record.
//! - [`ranking`] — the join of the sources, the order of the rows, and the
//!   totals of the header. Each process gets a row or a count.
//! - [`plan`] — the four rules of `faulte kill`, the order of the candidates,
//!   and the count of each session that the rules refused.
//! - [`stop`] — the question, the check immediately before each signal, and the
//!   sequence of `SIGTERM` and `SIGKILL`.
//! - [`render`] — the text that a person reads. Nothing here prints.
//! - [`machine`] — every fact of this Mac, behind one trait. The module
//!   `machine::macos` is the one part of `faulte` that runs a command or calls
//!   the kernel.

pub mod duration;
pub mod machine;
pub mod pid;
pub mod plan;
pub mod ranking;
pub mod render;
pub mod state;
pub mod stop;
pub mod table;
pub mod top;
pub mod vm;
