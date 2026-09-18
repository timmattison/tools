//! The reader of this Mac: the one part of `faulte` that runs a command or
//! calls the kernel.
//!
//! Each read here is the read that the issue names, and each one has a reason
//! to be a command or a call:
//!
//! - `/usr/bin/top` samples the page faults. It carries the entitlement
//!   `com.apple.system-task-ports.read`, so it reads the counter of every
//!   process of every account. A binary of this workspace cannot get that
//!   entitlement.
//! - `/bin/ps` reads the process table. It is set-user-ID root and carries the
//!   same entitlement, so it gives the owner, the parent, the memory, the
//!   start time and the command of every process. `sysinfo` gives none of
//!   those for a process of another account.
//! - `host_statistics64` and `vm.swapusage` give the counters of the virtual
//!   memory system. No command prints them as numbers that a parser can read.
//! - `occ` reads the Claude Code role of each process and the registry record
//!   of each session. It is the one reader of the registry in this workspace.
//!
//! Nothing here is a pure function, so no unit test covers this module. The
//! one test that reads this Mac runs the built binary and holds the shape of
//! the output. Every rule that this module feeds is a pure function with tests
//! of its own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::path::PathBuf;
use std::process::Command;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use occ::{classify, gather_processes, Role, SessionRecord, SessionRegistry};

use crate::duration::Span;
use crate::machine::{Machine, MachineError, Signal};
use crate::pid::{Pid, Uid};
use crate::ranking::{ClaudeRole, Viewer};
use crate::table::{self, ProcessRow};
use crate::top::{self, TopSample};
use crate::vm::{SwapUsage, VmCounters};

/// The `sysctl` that gives the largest number of processes that this Mac runs.
///
/// `top` prints the busiest processes alone unless a run asks for more, and
/// this number is the most that a Mac can have. Thus a run asks for it and
/// gets every process.
const MAX_PROCESSES: &str = "kern.maxproc";

/// The `sysctl` that gives the size of the swap file and how much of it is in
/// use.
const SWAP_USAGE: &str = "vm.swapusage";

/// The call that gives the counters of the virtual memory system.
const VM_STATISTICS: &str = "host_statistics64(HOST_VM_INFO64)";

/// The size of a page of memory that `faulte` takes when `sysconf` gives none.
///
/// It is the smallest page that any Mac uses. A Mac of Apple Silicon uses four
/// times as much. Thus a compressor size that this number makes is smaller
/// than the truth, and never larger than it. `sysconf` does not fail on any
/// Mac that Apple ships.
const SMALLEST_PAGE: u64 = 4096;

/// The text that an error states when a command wrote nothing to explain
/// itself.
const NO_MESSAGE: &str = "the command wrote no message";

/// The call that sends a signal to a process.
const KILL_CALL: &str = "kill";

/// The name of `SIGTERM`, for the message of an error.
const TERMINATE: &str = "SIGTERM";

/// The name of `SIGKILL`, for the message of an error.
const KILL: &str = "SIGKILL";

/// The first size of the buffer that `getpwuid_r` fills, in bytes.
const ACCOUNT_BUFFER: usize = 4096;

/// The largest size of that buffer. A record of an account that is larger than
/// this is not a record that `faulte` can use.
const LARGEST_ACCOUNT_BUFFER: usize = 64 * 1024;

/// What the account database says about one account.
#[derive(Debug, Clone, Default)]
struct Account {
    /// The name of the account, for example `tim`.
    name: Option<String>,
    /// The home directory of the account. The registry of the account is
    /// under it.
    home: Option<PathBuf>,
}

/// This Mac.
///
/// The account database answers slowly, and a Mac runs a thousand processes of
/// two accounts. Thus each answer stays in this value, and a run asks the
/// database once for each account.
pub struct Mac {
    /// What the account database said about each account that a read asked
    /// about.
    accounts: RefCell<HashMap<Uid, Account>>,
    /// The size of a page of memory in bytes.
    page_size: u64,
    /// The account that runs `faulte`.
    viewer: Viewer,
}

impl Mac {
    /// Reads the facts of this Mac that do not change while `faulte` runs.
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: `sysconf` reads one number of the operating system. It takes
        // no pointer and writes no memory of this process.
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        // SAFETY: `geteuid` reads one number of this process. It takes no
        // pointer and writes no memory.
        let uid = unsafe { libc::geteuid() };
        Self {
            accounts: RefCell::new(HashMap::new()),
            page_size: u64::try_from(page).unwrap_or(SMALLEST_PAGE),
            viewer: Viewer {
                uid: Uid::new(uid),
                is_root: uid == 0,
            },
        }
    }

    /// Gives what the account database says about `uid`, and remembers it.
    fn account(&self, uid: Uid) -> Account {
        if let Some(account) = self.accounts.borrow().get(&uid) {
            return account.clone();
        }
        let account = read_account(uid);
        self.accounts.borrow_mut().insert(uid, account.clone());
        account
    }

    /// Gives the largest number of processes that this Mac runs.
    fn max_processes(&self) -> Result<u32, MachineError> {
        // SAFETY: `kern.maxproc` is one `c_int`, which holds no pointer and no
        // padding.
        let count: libc::c_int = unsafe { read_sysctl(MAX_PROCESSES) }?;
        u32::try_from(count).map_err(|_| MachineError::KernelRead {
            call: MAX_PROCESSES.to_owned(),
            reason: format!("the kernel gave the count {count}, which is not a count"),
        })
    }
}

impl Default for Mac {
    /// Gives the same value as [`Mac::new`].
    fn default() -> Self {
        Self::new()
    }
}

impl Machine for Mac {
    fn sample_faults(&self, interval: Span) -> Result<(TopSample, Duration), MachineError> {
        let arguments = top::arguments(interval, self.max_processes()?);
        let started = Instant::now();
        let output = run(top::PROGRAM, &arguments, &[])?;
        let wall = started.elapsed();
        Ok((top::parse(&output)?, wall))
    }

    fn process_table(&self) -> Result<Vec<ProcessRow>, MachineError> {
        let output = run(table::PROGRAM, &table::arguments(), &table::ENVIRONMENT)?;
        Ok(table::parse(&output)?)
    }

    fn claude_roles(&self) -> HashMap<Pid, ClaudeRole> {
        gather_processes()
            .iter()
            .filter_map(|fact| {
                let role = match classify(fact) {
                    Role::Session => ClaudeRole::Session,
                    Role::Unreadable => ClaudeRole::Unreadable,
                    // A support process and a tool are Claude Code images, and
                    // neither one is a session that a person talks to.
                    Role::Support(_) | Role::SpawnedTool | Role::Unrelated => return None,
                };
                Some((Pid::new(fact.pid), role))
            })
            .collect()
    }

    fn record_for(
        &self,
        pid: Pid,
        owner: Uid,
        started_at_epoch_secs: u64,
    ) -> Option<SessionRecord> {
        let home = self.account(owner).home?;
        SessionRegistry::for_home(&home).record_for(pid.get(), started_at_epoch_secs)
    }

    fn vm_counters(&self) -> Result<VmCounters, MachineError> {
        let statistics = read_vm_statistics()?;
        Ok(VmCounters {
            swapins: statistics.swapins,
            swapouts: statistics.swapouts,
            compressor_pages: u64::from(statistics.compressor_page_count),
        })
    }

    fn swap_usage(&self) -> Result<SwapUsage, MachineError> {
        // SAFETY: `vm.swapusage` is one `xsw_usage`, which holds three 64-bit
        // counts and two 32-bit numbers. It holds no pointer.
        let usage: libc::xsw_usage = unsafe { read_sysctl(SWAP_USAGE) }?;
        Ok(SwapUsage {
            total_bytes: usage.xsu_total,
            used_bytes: usage.xsu_used,
        })
    }

    fn page_size(&self) -> u64 {
        self.page_size
    }

    fn viewer(&self) -> Viewer {
        self.viewer
    }

    fn account_name(&self, uid: Uid) -> Option<String> {
        self.account(uid).name
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn signal(&self, pid: Pid, signal: Signal) -> Result<(), MachineError> {
        let (number, name) = match signal {
            Signal::Terminate => (libc::SIGTERM, TERMINATE),
            Signal::Kill => (libc::SIGKILL, KILL),
        };
        let call = || format!("{KILL_CALL}({pid}, {name})");
        // The kernel counts a PID as a signed number, and a PID of a Mac is
        // far below the largest one. A number that does not fit is not a PID
        // at all, and a negative first argument of `kill` names a group of
        // processes.
        let target = libc::pid_t::try_from(pid.get()).map_err(|_| MachineError::SignalFailed {
            call: call(),
            reason: format!("the number {pid} is not a PID"),
        })?;
        // SAFETY: `kill` takes two numbers of the C language. It takes no
        // pointer and writes no memory of this process.
        let answer = unsafe { libc::kill(target, number) };
        if answer != 0 {
            return Err(MachineError::SignalFailed {
                call: call(),
                reason: io::Error::last_os_error().to_string(),
            });
        }
        Ok(())
    }

    fn sleep(&self, how_long: Duration) {
        thread::sleep(how_long);
    }
}

/// Runs `program` and gives what it wrote on its standard output.
///
/// A command that does not start and a command that fails are two different
/// errors, because the repair of each one is different. A `top` that is not
/// there is a Mac that lost a file of the system. A `top` that fails names the
/// argument that it refused.
fn run(
    program: &str,
    arguments: &[String],
    environment: &[(&str, &str)],
) -> Result<String, MachineError> {
    let output = Command::new(program)
        .args(arguments)
        .envs(environment.iter().copied())
        .output()
        .map_err(|error| MachineError::CommandDidNotStart {
            program: program.to_owned(),
            reason: error.to_string(),
        })?;
    if !output.status.success() {
        return Err(MachineError::CommandFailed {
            program: program.to_owned(),
            status: output.status.to_string(),
            message: first_line(&output.stderr),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Gives the first line that a command wrote, for an error message.
///
/// One line is enough to name the fault, and a command that failed can write
/// many lines. The bytes are read without a check of UTF-8, because a message
/// that `faulte` cannot read is still a message that a person can.
fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    line.unwrap_or(NO_MESSAGE).to_owned()
}

/// Reads the counters of the virtual memory system of this Mac.
fn read_vm_statistics() -> Result<libc::vm_statistics64, MachineError> {
    // The kernel counts in units of `integer_t`, and it refuses a count that
    // is smaller than the structure that it fills. A kernel that fills less
    // than this structure holds writes the count that it filled back.
    let units = size_of::<libc::vm_statistics64>() / size_of::<libc::integer_t>();
    let mut count = u32::try_from(units).map_err(|_| MachineError::KernelRead {
        call: VM_STATISTICS.to_owned(),
        reason: "the structure of the counters does not fit in a count".to_owned(),
    })?;
    // A structure of zeros is a valid value of every field, so a field that
    // the kernel does not fill reads as zero and never as memory of this
    // process that nobody wrote.
    let mut statistics = MaybeUninit::<libc::vm_statistics64>::zeroed();
    #[allow(
        deprecated,
        reason = "libc asks for the mach2 crate, which this workspace does not take. The call is the one entrance to the counters of the virtual memory system"
    )]
    // SAFETY: `mach_host_self` gives the port of this host, which is the port
    // that `host_statistics64` takes. The pointer names one whole structure of
    // this function, and `count` states its size in the units that the call
    // reads. The call writes no more than that.
    let answer = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            statistics.as_mut_ptr().cast::<libc::integer_t>(),
            &mut count,
        )
    };
    if answer != libc::KERN_SUCCESS {
        return Err(MachineError::KernelRead {
            call: VM_STATISTICS.to_owned(),
            reason: format!("the call ended with {answer}"),
        });
    }
    // SAFETY: the call answered with success, and every field that it did not
    // write holds the zero that this function wrote first.
    Ok(unsafe { statistics.assume_init() })
}

/// Reads the value of the `sysctl` `name`.
///
/// # Safety
///
/// `T` must be the type that the kernel writes for `name`. It must hold no
/// pointer and no padding, because the call writes raw bytes over it. A value
/// of `T` that is all zeros must be a valid value of `T`.
unsafe fn read_sysctl<T>(name: &str) -> Result<T, MachineError> {
    let key = CString::new(name).map_err(|_| MachineError::KernelRead {
        call: name.to_owned(),
        reason: "the name of the sysctl holds a zero byte".to_owned(),
    })?;
    // A value of zeros is a valid value of `T`, which the caller promises.
    let mut value = MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>();
    // SAFETY: the pointer names one whole value of `T` of this function, and
    // `size` states how many bytes the call can write. The last two arguments
    // are the value to write, and this call writes nothing.
    let answer = unsafe {
        libc::sysctlbyname(
            key.as_ptr(),
            value.as_mut_ptr().cast::<libc::c_void>(),
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    if answer != 0 {
        return Err(MachineError::KernelRead {
            call: name.to_owned(),
            reason: io::Error::last_os_error().to_string(),
        });
    }
    if size != size_of::<T>() {
        return Err(MachineError::KernelRead {
            call: name.to_owned(),
            reason: format!(
                "the kernel wrote {size} bytes, and the value holds {}",
                size_of::<T>()
            ),
        });
    }
    // SAFETY: the call answered with success and wrote the whole value.
    Ok(unsafe { value.assume_init() })
}

/// Reads the name and the home directory of the account `uid`.
///
/// An account that the database does not know gives a record of nothing. The
/// row of such a process then states the number of the account, and `faulte`
/// asks no registry for it. A Mac holds accounts that no person logs in to,
/// and each one of them can own a process.
fn read_account(uid: Uid) -> Account {
    let mut size = ACCOUNT_BUFFER;
    loop {
        let mut buffer = vec![0_u8; size];
        let mut record = MaybeUninit::<libc::passwd>::zeroed();
        let mut found: *mut libc::passwd = ptr::null_mut();
        // SAFETY: the record and the buffer are two values of this function,
        // and `buffer.len()` states how many bytes the call can write into the
        // buffer. The call writes the address of the record into `found`, or a
        // null pointer when it knows no such account.
        let answer = unsafe {
            libc::getpwuid_r(
                uid.get(),
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
                &mut found,
            )
        };
        // The buffer was too small. Twice the size is the usual repair, and a
        // record that is larger than the largest buffer is a record that
        // `faulte` does not use.
        if answer == libc::ERANGE && size < LARGEST_ACCOUNT_BUFFER {
            size *= 2;
            continue;
        }
        if answer != 0 || found.is_null() {
            return Account::default();
        }
        // SAFETY: the call answered with success and it wrote the record.
        let record = unsafe { record.assume_init() };
        // SAFETY: each of the two pointers is null, or it names a text inside
        // `buffer` that ends with a zero byte. The buffer is alive here, and
        // each text becomes a `String` of its own before the buffer goes.
        return unsafe {
            Account {
                name: text_of(record.pw_name),
                home: text_of(record.pw_dir).map(PathBuf::from),
            }
        };
    }
}

/// Gives the text that `pointer` names, or `None` when the pointer is null.
///
/// # Safety
///
/// `pointer` must be null, or it must name a text that ends with a zero byte
/// and that stays where it is for the whole of this call.
unsafe fn text_of(pointer: *const libc::c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the caller promises that the pointer names a text that ends with
    // a zero byte.
    let text = unsafe { CStr::from_ptr(pointer) };
    Some(text.to_string_lossy().into_owned())
}
