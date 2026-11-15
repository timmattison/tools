//! The reader of the kernel: the CPU tick counters and the memory pressure.

use std::ffi::CStr;
use std::fmt;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::ptr;

use crate::cpu::Ticks;
use crate::sample::Reading;

/// The name of the call that reads the CPU tick counters.
const CPU_CALL: &str = "host_statistics";

/// The `sysctl` that holds the pressure level of the memory.
const PRESSURE_LEVEL: &CStr = c"kern.memorystatus_vm_pressure_level";

/// The `sysctl` that holds the percentage of the memory that is free.
const FREE_PERCENT: &CStr = c"kern.memorystatus_level";

/// The index of the user ticks in `cpu_ticks`.
const USER_INDEX: usize = libc::CPU_STATE_USER as usize;

/// The index of the system ticks in `cpu_ticks`.
const SYSTEM_INDEX: usize = libc::CPU_STATE_SYSTEM as usize;

/// The index of the idle ticks in `cpu_ticks`.
const IDLE_INDEX: usize = libc::CPU_STATE_IDLE as usize;

/// The index of the nice ticks in `cpu_ticks`.
const NICE_INDEX: usize = libc::CPU_STATE_NICE as usize;

/// The reason that a read of the kernel failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    /// The name of the kernel call that failed.
    call: &'static str,
    /// What went wrong, in words.
    reason: String,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot read {}: {}", self.call, self.reason)
    }
}

impl std::error::Error for ReadError {}

/// A reader of the kernel values that `glance` shows.
#[derive(Debug)]
pub struct Reader {
    /// The port of this host. `new` asks for it once.
    host: libc::mach_port_t,
}

impl Reader {
    /// Makes a reader.
    ///
    /// The reader calls `mach_host_self` once and keeps the port. Each call
    /// adds a user reference to the host port right. At one sample each
    /// second, a call on each read can reach the limit of the reference count
    /// within a day.
    #[must_use]
    pub fn new() -> Reader {
        #[expect(
            deprecated,
            reason = "libc asks for the mach2 crate, which this workspace does not take. The call is the one entrance to the port of the host"
        )]
        // SAFETY: `mach_host_self` takes no argument and gives the port of
        // this host.
        let host = unsafe { libc::mach_host_self() };
        Reader { host }
    }

    /// Reads the CPU tick counters and the memory pressure.
    ///
    /// # Errors
    ///
    /// Returns an error if a kernel call fails or gives a value of the wrong size.
    pub fn read(&self) -> Result<Reading, ReadError> {
        let ticks = self.read_ticks()?;
        Ok(Reading {
            ticks,
            pressure_level: read_i32_sysctl(PRESSURE_LEVEL)?,
            free_percent: read_i32_sysctl(FREE_PERCENT)?,
        })
    }

    /// Reads the CPU tick counters of the host.
    fn read_ticks(&self) -> Result<Ticks, ReadError> {
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        // A structure of zeros is a valid value of every field.
        let mut info = MaybeUninit::<libc::host_cpu_load_info>::zeroed();
        // SAFETY: `self.host` is the port of this host. The pointer names one
        // whole `host_cpu_load_info` of this function, and `count` states its
        // size in the units that the call reads. The call writes no more.
        let answer = unsafe {
            libc::host_statistics(
                self.host,
                libc::HOST_CPU_LOAD_INFO,
                info.as_mut_ptr().cast::<libc::integer_t>(),
                &mut count,
            )
        };
        if answer != libc::KERN_SUCCESS {
            return Err(ReadError {
                call: CPU_CALL,
                reason: format!("the call ended with {answer}"),
            });
        }
        // SAFETY: the call answered with success, and every field that it did
        // not write holds the zero that this function wrote first.
        let info = unsafe { info.assume_init() };
        Ok(Ticks {
            user: info.cpu_ticks[USER_INDEX],
            system: info.cpu_ticks[SYSTEM_INDEX],
            idle: info.cpu_ticks[IDLE_INDEX],
            nice: info.cpu_ticks[NICE_INDEX],
        })
    }
}

/// Reads the `i32` value of the `sysctl` `name`.
fn read_i32_sysctl(name: &'static CStr) -> Result<i32, ReadError> {
    let call = name.to_str().unwrap_or("sysctl");
    let mut value: i32 = 0;
    let mut size = size_of::<i32>();
    // SAFETY: `name` is a C string. The pointer names one `i32` of this
    // function, and `size` states how many bytes the call can write. The last
    // two arguments are the value to write, and this call writes nothing.
    let answer = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            ptr::from_mut(&mut value).cast::<libc::c_void>(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if answer != 0 {
        return Err(ReadError {
            call,
            reason: io::Error::last_os_error().to_string(),
        });
    }
    if size != size_of::<i32>() {
        return Err(ReadError {
            call,
            reason: format!(
                "the kernel wrote {size} bytes, and the value holds {}",
                size_of::<i32>()
            ),
        });
    }
    Ok(value)
}

impl Default for Reader {
    fn default() -> Reader {
        Reader::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The raw pressure levels of the kernel.
    const LEVELS: [i32; 3] = [1, 2, 4];

    #[test]
    fn two_reads_of_this_mac_give_levels_and_percentages_in_range() {
        let reader = Reader::new();
        for _ in 0..2 {
            let reading = reader.read().expect("the kernel answers");
            assert!(
                LEVELS.contains(&reading.pressure_level),
                "level {}",
                reading.pressure_level
            );
            assert!(
                (0..=100).contains(&reading.free_percent),
                "free {}",
                reading.free_percent
            );
        }
    }
}
