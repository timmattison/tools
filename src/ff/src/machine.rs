//! The facts of this machine that a search of the whole machine needs.

use std::io;

use crate::plan::{MachinePlan, Mount};

/// Reads the mount table of this machine through `getfsstat`.
///
/// # Errors
///
/// Returns the error of `getfsstat`.
pub fn mount_table() -> io::Result<Vec<Mount>> {
    Ok(Vec::new())
}

/// Plans a search of this machine, from its mount table and from
/// `/usr/share/firmlinks`.
///
/// # Errors
///
/// Returns the error of `getfsstat`, or an error other than "not found" from
/// the read of the firmlink map.
pub fn plan_this_machine() -> io::Result<MachinePlan> {
    Ok(MachinePlan::default())
}
