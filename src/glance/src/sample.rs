//! One sample of the host: the CPU share and the memory pressure.

use crate::cpu::{CpuShare, Ticks};
use crate::pressure::Pressure;

/// One reading of the kernel. Raw values, as the reader gets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// The CPU tick counters.
    pub ticks: Ticks,
    /// The raw value of `kern.memorystatus_vm_pressure_level`.
    pub pressure_level: i32,
    /// The raw value of `kern.memorystatus_level`, the percentage that is free.
    pub free_percent: i32,
}

/// One column of each chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// The CPU share between two readings.
    pub cpu: CpuShare,
    /// The memory pressure of the later reading.
    pub pressure: Pressure,
}

impl Sample {
    /// The CPU share between the two readings, and the pressure of `later`.
    #[must_use]
    pub fn between(earlier: &Reading, later: &Reading) -> Sample {
        Sample {
            cpu: CpuShare::between(earlier.ticks, later.ticks),
            pressure: Pressure::from_raw(later.pressure_level, later.free_percent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pressure::Level;

    const TOLERANCE: f64 = 1e-12;

    #[test]
    fn a_sample_takes_the_cpu_between_the_readings_and_the_pressure_of_the_later_one() {
        let earlier = Reading {
            ticks: Ticks::default(),
            pressure_level: 1,
            free_percent: 90,
        };
        let later = Reading {
            ticks: Ticks {
                user: 50,
                system: 25,
                idle: 25,
                nice: 0,
            },
            pressure_level: 4,
            free_percent: 10,
        };
        let sample = Sample::between(&earlier, &later);
        assert!((sample.cpu.user() - 0.5).abs() < TOLERANCE);
        assert!((sample.cpu.system() - 0.25).abs() < TOLERANCE);
        assert!((sample.cpu.idle() - 0.25).abs() < TOLERANCE);
        assert_eq!(sample.pressure.level(), Level::Critical);
        assert_eq!(sample.pressure.percent_in_use(), 90);
    }
}
