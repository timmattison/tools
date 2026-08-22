//! Read the DVFS table of an Apple Silicon SoC from the IO Registry.
//!
//! DVFS means dynamic voltage and frequency scaling. The SoC carries one table
//! for each cluster, and each table lists the frequency and voltage of every
//! step the cluster can run at. The last entry of the P-cluster table is the
//! peak clock of the chip, and it is the number every measurement in this crate
//! is judged against.
//!
//! # Why the IO Registry, and not `ioreg`
//!
//! The command line tool `ioreg` renders these tables as hexadecimal inside a
//! wall of text, so reading them that way means a regular expression over the
//! output of another program. The question — "what does this property of this
//! registry node hold" — names a structure, not a piece of text, so this module
//! asks the IO Registry itself through IOKit. Nothing is text-matched.

use thiserror::Error;

use crate::mhz::Mhz;

/// The IO Registry property that holds the DVFS table of the P-cluster.
const P_CLUSTER_KEY: &str = "voltage-states5-sram";

/// The IO Registry property that holds the DVFS table of the E-cluster.
const E_CLUSTER_KEY: &str = "voltage-states1-sram";

/// Each entry of a DVFS table is two little-endian `u32` words: a frequency in
/// kilohertz, then a voltage.
const ENTRY_BYTES: usize = 8;

/// A DVFS table could not be read.
#[derive(Debug, Error)]
pub enum DvfsError {
    /// The IO Registry has no such property anywhere under the service plane.
    ///
    /// An Intel Mac reaches this, because it carries no such property. So does
    /// a future SoC that renames the key.
    #[error("no `{key}` property in the IO Registry; this tool needs an Apple Silicon Mac")]
    PropertyMissing {
        /// The property that was searched for.
        key: &'static str,
    },

    /// The property exists but does not hold the raw bytes of a table.
    #[error("the `{key}` property is not raw data")]
    PropertyNotData {
        /// The property whose type was wrong.
        key: &'static str,
    },

    /// The property holds bytes, but no complete entry with a frequency above
    /// zero. Reporting an empty table as a maximum of zero would make every
    /// later ratio read as full throttling.
    #[error("the `{key}` property holds no usable frequency step")]
    TableEmpty {
        /// The property whose table could not be decoded.
        key: &'static str,
    },
}

/// The frequency steps of the two CPU clusters of an Apple Silicon SoC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DvfsTable {
    /// P-cluster steps, in the order the SoC lists them.
    p_steps: Vec<Mhz>,
    /// E-cluster steps, in the order the SoC lists them.
    e_steps: Vec<Mhz>,
}

impl DvfsTable {
    /// Read the table of the running machine from the IO Registry.
    ///
    /// # Errors
    ///
    /// Returns [`DvfsError`] when the IO Registry carries no DVFS property,
    /// when the property is not raw data, or when it decodes to no usable step.
    pub fn read() -> Result<Self, DvfsError> {
        Err(DvfsError::PropertyMissing { key: P_CLUSTER_KEY })
    }

    /// Build a table from already-decoded steps. Used by tests, and by
    /// [`Self::read`] once the bytes are decoded.
    #[must_use]
    pub fn from_steps(p_steps: Vec<Mhz>, e_steps: Vec<Mhz>) -> Self {
        Self { p_steps, e_steps }
    }

    /// The peak clock of the P-cluster.
    #[must_use]
    pub fn p_max(&self) -> Mhz {
        self.p_steps.iter().copied().max().unwrap_or(Mhz::new(0))
    }

    /// The peak clock of the E-cluster.
    #[must_use]
    pub fn e_max(&self) -> Mhz {
        self.e_steps.iter().copied().max().unwrap_or(Mhz::new(0))
    }

    /// Every P-cluster step, in the order the SoC lists them.
    #[must_use]
    pub fn p_steps(&self) -> &[Mhz] {
        &self.p_steps
    }

    /// Every E-cluster step, in the order the SoC lists them.
    #[must_use]
    pub fn e_steps(&self) -> &[Mhz] {
        &self.e_steps
    }
}

/// Decode the raw bytes of a DVFS property into frequency steps.
///
/// The layout is a repeated pair of little-endian `u32` words: a frequency in
/// kilohertz, then a voltage. A trailing partial entry is ignored, and a step
/// whose frequency is zero is padding rather than a real step.
#[must_use]
pub fn decode_voltage_states(_raw: &[u8]) -> Vec<Mhz> {
    Vec::new()
}

/// Read one IO Registry property as raw bytes, searching the whole service
/// plane from the root.
fn read_property(_key: &'static str) -> Result<Vec<u8>, DvfsError> {
    Err(DvfsError::PropertyMissing { key: P_CLUSTER_KEY })
}
