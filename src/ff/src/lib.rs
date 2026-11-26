//! `ff` finds files by name across a whole machine.
//!
//! BSD `find` abandons the complete walk when a system call under `fts_read`
//! returns `EINTR`. `ff` reads each directory with its own calls, and it does
//! an interrupted call again in place. A walk thus never loses the paths that
//! it did not print yet.

pub mod fs;
#[cfg(target_os = "macos")]
pub mod machine;
pub mod pattern;
pub mod plan;
pub mod summary;
pub mod walk;
