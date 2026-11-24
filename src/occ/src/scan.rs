//! Reading the machine: the processes running on it.
//!
//! This is the only module that talks to the operating system. Everything it
//! learns is handed to the rules in [`crate::process`] and [`crate::report`] as
//! plain values.

use crate::ProcessFact;
use std::path::Path;

/// Reads the kernel accounting name of a process.
///
/// This is the basename of the file the process actually executed, recorded
/// when it started. For Claude Code it is the release number, and it is the only
/// reliable source of that number: the executable path of a running session
/// resolves through the `claude` launcher, which is a link to whichever release
/// is installed *now*. Reading the release from that path would report the
/// newest release for a session running a release from months ago — the exact
/// claim this tool exists to make, made backwards.
///
/// Returns `None` when the name cannot be read, which is the normal answer for
/// another account's process.
#[must_use]
pub fn accounting_name(pid: u32) -> Option<String> {
    read_accounting_name(pid)
}

/// Reads the accounting name from the kernel's process information.
///
/// `pbi_name` is preferred over `pbi_comm` because the kernel truncates `pbi_comm`
/// at sixteen characters, and the longer field carries the same value untruncated.
#[cfg(target_os = "macos")]
fn read_accounting_name(pid: u32) -> Option<String> {
    // A process identifier is signed at this interface, and identifiers that do
    // not fit are not identifiers this tool can ask about.
    let pid = i32::try_from(pid).ok()?;

    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let wanted = std::mem::size_of::<libc::proc_bsdinfo>();

    // SAFETY: `proc_pidinfo` writes at most `wanted` bytes into the buffer, and
    // `wanted` is that buffer's own size. The call is read-only with respect to
    // the target process and reports how many bytes it filled, which is checked
    // below before the buffer is read.
    let filled = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            i32::try_from(wanted).ok()?,
        )
    };
    if usize::try_from(filled).ok()? != wanted {
        return None;
    }

    // SAFETY: the call above reported that it filled the whole structure.
    let info = unsafe { info.assume_init() };

    read_c_name(&info.pbi_name).or_else(|| read_c_name(&info.pbi_comm))
}

/// Reads a NUL-terminated name out of a fixed-size kernel field.
///
/// The field carries the bytes of the name, and `libc::c_char` is signed on this
/// platform, so a byte above 0x7F arrives as a negative number. Each element is
/// therefore reinterpreted, not converted: a numeric conversion would fold such
/// a byte to a different value, and the folded value is printable ASCII, so the
/// UTF-8 check below would accept the wrong name and report no error.
///
/// Returns `None` when the field is empty or the bytes are not UTF-8.
#[cfg(target_os = "macos")]
fn read_c_name(field: &[libc::c_char]) -> Option<String> {
    let bytes: Vec<u8> = field
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| byte.cast_unsigned())
        .collect();
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Reads the accounting name from `/proc/<pid>/comm`.
#[cfg(target_os = "linux")]
fn read_accounting_name(pid: u32) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = name.trim_end_matches('\n').to_string();
    (!name.is_empty()).then_some(name)
}

/// Reports that no accounting name is available on this platform.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn read_accounting_name(_pid: u32) -> Option<String> {
    None
}

/// Reads every process on the machine.
///
/// Returns all of them, not only the Claude Code ones: deciding what a process
/// is belongs to [`crate::classify`], and keeping that decision out of this
/// module is what lets it be tested without a live process table.
#[must_use]
pub fn gather_processes() -> Vec<ProcessFact> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessFact {
            pid: pid.as_u32(),
            // The kernel's name is preferred over the one `sysinfo` reports,
            // which is the basename of `argv[0]` and says `claude` for every
            // release. Falling back to it keeps a name available on platforms
            // where the kernel's is not readable.
            accounting_name: accounting_name(pid.as_u32())
                .unwrap_or_else(|| process.name().to_string_lossy().into_owned()),
            exe: process.exe().map(Path::to_path_buf),
            argv: process
                .cmd()
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            cwd: process.cwd().map(Path::to_path_buf),
            uptime_secs: process.run_time(),
            start_time_epoch_secs: process.start_time(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::read_c_name;
    use super::{accounting_name, gather_processes};

    #[test]
    fn reads_the_accounting_name_of_this_process() {
        // The accounting name is the basename of the executed file, truncated by
        // the kernel. Checking it against this test binary's own path is a
        // ground truth available on any machine.
        let executable = std::env::current_exe().expect("current executable");
        let basename = executable
            .file_name()
            .and_then(|n| n.to_str())
            .expect("executable basename");

        let found = accounting_name(std::process::id()).expect("own accounting name");

        assert!(!found.is_empty(), "the accounting name should not be empty");
        assert!(
            basename.starts_with(&found),
            "accounting name {found:?} should be a prefix of the executable name {basename:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_kernel_name_keeps_its_non_ascii_bytes() {
        // The kernel field holds the bytes of the name, and `libc::c_char` is a
        // signed type on this platform, so every byte above 0x7F arrives as a
        // negative number. The name must be read back from those bit patterns.
        // A magnitude reads a different name: the absolute value of 0xC3 is
        // 0x3D and the absolute value of 0xA9 is 0x57, so "café" becomes
        // "caf=W". Both of those are printable ASCII, so the UTF-8 check that
        // follows accepts the wrong name and reports no error.
        let name = "café";
        let mut field: [libc::c_char; 16] = [0; 16];
        for (slot, byte) in field.iter_mut().zip(name.as_bytes()) {
            *slot = byte.cast_signed();
        }

        assert_eq!(read_c_name(&field), Some(name.to_owned()));
    }

    #[test]
    fn the_accounting_name_of_a_process_that_is_not_running_is_absent() {
        // Process identifier 0 is never an ordinary process to be read.
        assert_eq!(accounting_name(0), None);
    }

    #[test]
    fn the_gathered_release_does_not_come_from_the_launcher_link() {
        // The regression this guards: the executable path of a session resolves
        // through the `claude` launcher link, so reading the release from it
        // reports the installed release rather than the running one. The
        // accounting name of this test process is not the launcher's name, and
        // that is what the gathered facts must carry.
        let mine = std::process::id();
        let gathered = gather_processes();
        let fact = gathered
            .iter()
            .find(|fact| fact.pid == mine)
            .expect("this process should be gathered");

        let expected = accounting_name(mine).expect("own accounting name");
        assert_eq!(fact.accounting_name, expected);
    }

    #[test]
    fn gathers_the_running_process_table() {
        // The one process guaranteed to be running is this test.
        let mine = std::process::id();
        let gathered = gather_processes();
        assert!(
            gathered.iter().any(|fact| fact.pid == mine),
            "the running test process should appear in the gathered table"
        );
    }
}
