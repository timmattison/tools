//! The reader of this Mac.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use faulte::pid::Pid;
use faulte::vm::{SwapUsage, VmCounters};

use crate::machine::{Machine, SourceError};
use crate::sample::ProcessRow;
use crate::verdict::{CpuTicks, Pressure};

/// This Mac.
pub struct Mac;

impl Mac {
    /// Makes the reader.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for Mac {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for Mac {
    fn process_table(&self) -> Result<Vec<ProcessRow>, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn cpu_ticks(&self) -> Result<CpuTicks, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn vm_counters(&self) -> Result<VmCounters, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn swap_usage(&self) -> Result<SwapUsage, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn pressure(&self) -> Result<Pressure, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn load(&self) -> Result<f64, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn processors(&self) -> Result<u32, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn page_size(&self) -> u64 {
        4096
    }
    fn footprint(&self, _pid: Pid) -> Option<u64> {
        None
    }
    fn claude_pids(&self) -> HashSet<Pid> {
        HashSet::new()
    }
    fn arguments(&self) -> Result<HashMap<Pid, String>, SourceError> {
        Err(SourceError::NoArguments)
    }
    fn holds_git(&self, _directory: &Path) -> bool {
        false
    }
    fn sleep(&self, _how_long: Duration) {}
    fn clock(&self) -> Duration {
        Duration::ZERO
    }
}
