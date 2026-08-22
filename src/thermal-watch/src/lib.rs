//! `thermal-watch` shows whether an Apple Silicon Mac decreases its clock under
//! sustained load.
//!
//! macOS reports two different signals, and only one of them answers the
//! question:
//!
//! 1. **Thermal pressure** (`Nominal`, `Fair`, `Serious`, `Critical`). The OS
//!    raises this level to tell applications to do less work. Apple Silicon
//!    decreases its clock long before the level changes, so `Nominal` under
//!    full load does not mean full speed. Every process-level tool that reports
//!    "thermals nominal" reads this signal.
//! 2. **The measured P-cluster frequency**, against the maximum frequency in
//!    the DVFS table of the chip. This is the ground truth, and it is what this
//!    tool judges on.
//!
//! The two signals disagree often, and the disagreement is the point: a machine
//! can sit at `Nominal` while its P-cores run 20% below their peak clock.

pub mod dvfs;
pub mod load;
pub mod mhz;
pub mod powermetrics;
pub mod render;
pub mod report;

pub use dvfs::{DvfsError, DvfsTable};
pub use load::Load;
pub use mhz::Mhz;
pub use powermetrics::{PressureLevel, Sample, SampleStream};
pub use report::{Outcome, Verdict};
