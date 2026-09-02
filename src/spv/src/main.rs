//! spv - Smart process viewer with enhanced filtering and display
//!
//! A CLI tool that provides enhanced process listing with flexible filtering
//! and display options.

use std::collections::HashMap;
use std::ffi::CStr;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use human_bytes::human_bytes;
use regex::Regex;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

/// Cached result of lsof availability check.
///
/// This prevents repeated warnings when lsof is not found.
static LSOF_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Smart process viewer with enhanced filtering and display.
///
/// # Examples
///
/// ```text
/// spv 77763           - Show process with PID 77763
/// spv 77763,82313     - Show multiple PIDs
/// spv node            - Find processes containing 'node'
/// spv --regex 'node.*' - Find with regex
/// spv --cwd zsh       - Show processes with their working directories
/// spv --lsof $$       - Show open files for current shell
/// ```
#[derive(Parser)]
#[command(
    name = "spv",
    version = version_string!(),
    about = "Smart process viewer with enhanced filtering and display",
    long_about = "Examples:\n  spv 77763           - Show process with PID 77763\n  spv 77763,82313     - Show multiple PIDs\n  spv node            - Find processes containing 'node'\n  spv --regex 'node.*' - Find with regex\n  spv --cwd zsh       - Show processes with their CWD\n  spv --lsof $$       - Show open files for process"
)]
struct Args {
    /// PID(s) or name pattern to match.
    ///
    /// Can be a single PID, comma-separated PIDs, or a name pattern.
    #[arg(required = true)]
    pattern: String,

    /// Use regex matching instead of substring.
    ///
    /// When enabled, the pattern is treated as a regular expression.
    /// Note: Without this flag, name matching is case-insensitive.
    #[arg(long)]
    regex: bool,

    /// Show current working directory.
    ///
    /// Adds a CWD column showing each process's working directory.
    #[arg(long)]
    cwd: bool,

    /// Show open files (uses lsof).
    ///
    /// Lists all files opened by matching processes.
    #[arg(long)]
    lsof: bool,

    /// Respect case in the name search.
    ///
    /// Without this flag a substring search ignores case. A regular expression
    /// carries its own case rule in the `(?i)` inline flag, so this flag leaves
    /// `--regex` alone.
    #[arg(long)]
    case_sensitive: bool,

    /// Search the whole command line, not only the executable name.
    ///
    /// This is the reach that `pgrep -f` has.
    #[arg(long, short = 'f')]
    full: bool,

    /// Show environment variables.
    ///
    /// A value whose name reads like a credential is hidden. Add
    /// `--show-secrets` to print every value in full.
    #[arg(long)]
    env: bool,

    /// Print every environment value in full, credentials included.
    #[arg(long)]
    show_secrets: bool,

    /// Show network connections (uses lsof).
    #[arg(long)]
    net: bool,

    /// Show every section: working directory, open files, environment, network.
    #[arg(long)]
    all: bool,

    /// Raw output without table formatting.
    ///
    /// Produces columnar output similar to traditional ps.
    #[arg(long)]
    raw: bool,
}

/// The sections to print under the process table.
#[derive(Debug, PartialEq, Eq)]
struct Sections {
    cwd: bool,
    files: bool,
    env: bool,
    net: bool,
}

impl Args {
    /// Resolves which sections to print.
    ///
    /// # Returns
    ///
    /// The sections the flags asked for. `--all` turns on every one.
    fn sections(&self) -> Sections {
        Sections {
            cwd: false,
            files: false,
            env: false,
            net: false,
        }
    }
}

/// How a name pattern is compared against a process.
struct MatchOptions {
    /// Treat the pattern as a regular expression.
    use_regex: bool,
    /// Respect case in the substring search.
    case_sensitive: bool,
    /// Search the whole command line instead of the executable name.
    match_full_command: bool,
}

/// Represents the type of pattern provided by the user.
enum PatternType {
    /// A single process ID.
    SinglePid(u32),
    /// Multiple process IDs.
    MultiplePids(Vec<u32>),
    /// A name pattern (substring or regex).
    NamePattern(String),
}

/// Information about a single process.
///
/// # CPU Usage Note
///
/// The `cpu_usage` field represents a point-in-time snapshot. The sysinfo crate
/// typically requires two refresh calls with a delay between them for accurate
/// CPU percentage calculations. Since this tool performs a single snapshot for
/// responsiveness, the CPU value may be 0% or less accurate than tools that
/// continuously monitor processes. This is an intentional tradeoff - users who
/// need precise CPU tracking should use tools like `top` or `htop` instead.
struct ProcessInfo {
    pid: u32,
    name: String,
    user: String,
    cpu_usage: f32,
    memory: u64,
    status: String,
    command: String,
    cwd: Option<String>,
}

/// Information about an open file from lsof.
struct OpenFile {
    fd: String,
    file_type: String,
    name: String,
}

/// Parses the pattern to determine if it's a PID, multiple PIDs, or a name pattern.
///
/// # Arguments
///
/// * `pattern` - The pattern string from command line arguments
///
/// # Returns
///
/// The detected pattern type.
fn parse_pattern(pattern: &str) -> PatternType {
    // Check for comma-separated PIDs
    if pattern.contains(',') {
        let parts: Vec<&str> = pattern.split(',').collect();
        let pids: Result<Vec<u32>, _> = parts.iter().map(|p| p.trim().parse::<u32>()).collect();
        if let Ok(pids) = pids {
            return PatternType::MultiplePids(pids);
        }
    }

    // Check for single PID
    if let Ok(pid) = pattern.parse::<u32>() {
        return PatternType::SinglePid(pid);
    }

    // Otherwise it's a name pattern
    PatternType::NamePattern(pattern.to_string())
}

/// Gets the username for a given user ID on Unix systems.
///
/// # Arguments
///
/// * `uid` - The user ID to look up
///
/// # Returns
///
/// The username if found, otherwise the UID as a string.
#[cfg(unix)]
fn get_username(uid: u32) -> String {
    // SAFETY: getpwuid is a standard POSIX function that returns a pointer to
    // a passwd struct. The returned pointer is to static storage and should not
    // be freed. We immediately copy the data we need.
    //
    // NOTE: getpwuid is NOT thread-safe as it returns a pointer to static storage
    // that can be overwritten by subsequent calls. This is acceptable for this
    // single-threaded CLI tool, but this function should not be used in
    // multi-threaded contexts without synchronization.
    unsafe {
        let passwd = libc::getpwuid(uid);
        if passwd.is_null() {
            return uid.to_string();
        }
        let name = (*passwd).pw_name;
        if name.is_null() {
            return uid.to_string();
        }
        CStr::from_ptr(name).to_string_lossy().into_owned()
    }
}

/// Process information from sysctl on macOS.
///
/// The sysinfo crate uses proc_pidinfo() which requires elevated privileges to
/// get info for other users' processes. On macOS, sysctl(KERN_PROC_ALL) provides
/// basic process info (including UID and status) to all users without root.
/// This struct stores that fallback information.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct SysctlProcessInfo {
    uid: u32,
    status: u8,
}

// Constants for kinfo_proc struct layout on macOS (arm64/x86_64).
// These offsets are stable across macOS versions as they're part of the ABI.
// Verified via offsetof() in C: see sys/sysctl.h for struct definition.
#[cfg(target_os = "macos")]
mod kinfo_proc_layout {
    /// Size of struct kinfo_proc in bytes
    pub const SIZE: usize = 648;
    /// Offset of kp_proc.p_pid (i32)
    pub const OFFSET_PID: usize = 40;
    /// Offset of kp_proc.p_stat (i8)
    pub const OFFSET_STAT: usize = 36;
    /// Offset of kp_eproc.e_ucred.cr_uid (u32)
    pub const OFFSET_UID: usize = 420;
}

/// Gets process info for all processes using sysctl(KERN_PROC_ALL).
///
/// This provides UID and status for all processes without root privileges,
/// unlike proc_pidinfo which requires elevated privileges for other users' processes.
///
/// # Returns
///
/// A HashMap mapping PID to process info, or None if the sysctl call fails.
#[cfg(target_os = "macos")]
fn get_all_process_info_via_sysctl() -> Option<HashMap<u32, SysctlProcessInfo>> {
    use kinfo_proc_layout::{OFFSET_PID, OFFSET_STAT, OFFSET_UID, SIZE};

    // SAFETY: sysctl is a standard BSD system call. We first query the size needed,
    // allocate a buffer, then retrieve the data. We read values at known offsets
    // that are stable across macOS versions (part of the kernel ABI).
    unsafe {
        let mut mib: [libc::c_int; 3] = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL];
        let mib_len: libc::c_uint = 3;
        let mut size: libc::size_t = 0;

        // First call to get required buffer size
        if libc::sysctl(
            mib.as_mut_ptr(),
            mib_len,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }

        // Allocate buffer with extra space for new processes that may appear
        // between the size query and the actual data retrieval
        let extra_space = size / 10;
        size += extra_space;

        let mut buffer: Vec<u8> = vec![0; size];

        // Second call to get the actual data
        let mut actual_size = size;
        if libc::sysctl(
            mib.as_mut_ptr(),
            mib_len,
            buffer.as_mut_ptr().cast(),
            &mut actual_size,
            std::ptr::null_mut(),
            0,
        ) != 0
        {
            return None;
        }

        // Validate that the kernel returned data aligned to our expected struct size.
        // If this fails, the kinfo_proc layout has changed and our offsets are wrong.
        if !actual_size.is_multiple_of(SIZE) {
            return None;
        }

        let count = actual_size / SIZE;
        let mut map = HashMap::with_capacity(count);

        for i in 0..count {
            let base = i * SIZE;

            // Read pid at offset (i32, native endian)
            let Ok(pid_bytes): Result<[u8; 4], _> =
                buffer[base + OFFSET_PID..base + OFFSET_PID + 4].try_into()
            else {
                continue;
            };
            let pid = i32::from_ne_bytes(pid_bytes);

            if pid > 0 {
                // Read status at offset (i8/u8)
                let status = buffer[base + OFFSET_STAT];

                // Read uid at offset (u32, native endian)
                let Ok(uid_bytes): Result<[u8; 4], _> =
                    buffer[base + OFFSET_UID..base + OFFSET_UID + 4].try_into()
                else {
                    continue;
                };
                let uid = u32::from_ne_bytes(uid_bytes);

                #[expect(
                    clippy::cast_sign_loss,
                    reason = "PIDs are always non-negative, checked above"
                )]
                let pid_u32 = pid as u32;
                map.insert(pid_u32, SysctlProcessInfo { uid, status });
            }
        }

        Some(map)
    }
}

/// Converts a macOS process status code to a human-readable string.
///
/// These status codes are defined in sys/proc.h and represent the BSD process states.
#[cfg(target_os = "macos")]
fn status_code_to_string(status: u8) -> String {
    // Status codes from sys/proc.h:
    // SIDL = 1: Process being created
    // SRUN = 2: Currently runnable
    // SSLEEP = 3: Sleeping on an address
    // SSTOP = 4: Process debugging or suspension
    // SZOMB = 5: Awaiting collection by parent
    match status {
        1 => "Idle".to_string(),
        2 => "Run".to_string(),
        3 => "Sleep".to_string(),
        4 => "Stop".to_string(),
        5 => "Zombie".to_string(),
        _ => format!("Unknown({status})"),
    }
}

/// Collects process information based on the pattern.
///
/// # Arguments
///
/// * `system` - The sysinfo System instance
/// * `pattern` - The parsed pattern type
/// * `options` - How a name pattern is compared
/// * `include_cwd` - Whether to include CWD information
///
/// # Returns
///
/// A vector of matching process information.
///
/// # Errors
///
/// Returns an error if regex compilation fails.
fn collect_processes(
    system: &System,
    pattern: &PatternType,
    options: &MatchOptions,
    include_cwd: bool,
) -> Result<Vec<ProcessInfo>> {
    let mut processes = Vec::new();
    let regex = match pattern {
        PatternType::NamePattern(p) if options.use_regex => {
            Some(Regex::new(p).context("Invalid regex pattern")?)
        }
        _ => None,
    };

    // On macOS, get process info via sysctl as a fallback for when sysinfo
    // can't access other users' processes (proc_pidinfo requires elevated privileges)
    #[cfg(target_os = "macos")]
    let sysctl_info = get_all_process_info_via_sysctl().unwrap_or_default();

    for (pid, process) in system.processes() {
        let pid_u32 = pid.as_u32();
        let name = process.name().to_string_lossy().to_string();

        let command = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" ");

        let matches = match pattern {
            PatternType::SinglePid(p) => pid_u32 == *p,
            PatternType::MultiplePids(pids) => pids.contains(&pid_u32),
            PatternType::NamePattern(p) => matches_name_pattern(
                p,
                regex.as_ref(),
                &name,
                &command,
                options.match_full_command,
                options.case_sensitive,
            ),
        };

        if matches {
            let user = process
                .user_id()
                .map(|uid| {
                    // Platform-specific handling is inline because sysinfo's Uid type
                    // differs across platforms. On Unix, Uid implements Deref<Target = uid_t>,
                    // allowing us to call get_username(**uid). On non-Unix platforms,
                    // we fall back to displaying the UID directly via sysinfo's Display impl.
                    //
                    // NOTE: The non-Unix path relies on sysinfo::Uid implementing Display,
                    // which it does per the sysinfo API. This has not been tested on Windows
                    // but should work correctly.
                    #[cfg(unix)]
                    {
                        get_username(**uid)
                    }
                    #[cfg(not(unix))]
                    {
                        uid.to_string()
                    }
                })
                .or_else(|| {
                    // Fallback to sysctl data on macOS when sysinfo returns None
                    #[cfg(target_os = "macos")]
                    {
                        sysctl_info.get(&pid_u32).map(|info| get_username(info.uid))
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        None
                    }
                })
                .unwrap_or_else(|| "unknown".to_string());

            let status = {
                let sysinfo_status = format!("{:?}", process.status());
                // If sysinfo returns Unknown, try sysctl fallback on macOS
                if sysinfo_status.starts_with("Unknown") {
                    #[cfg(target_os = "macos")]
                    {
                        sysctl_info
                            .get(&pid_u32)
                            .map(|info| status_code_to_string(info.status))
                            .unwrap_or(sysinfo_status)
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        sysinfo_status
                    }
                } else {
                    sysinfo_status
                }
            };

            let cwd = if include_cwd {
                process.cwd().map(|p| p.to_string_lossy().to_string())
            } else {
                None
            };

            processes.push(ProcessInfo {
                pid: pid_u32,
                name,
                user,
                cpu_usage: process.cpu_usage(),
                memory: process.memory(),
                status,
                command,
                cwd,
            });
        }
    }

    // Sort by PID for consistent output
    processes.sort_by_key(|p| p.pid);
    Ok(processes)
}

/// Checks if lsof is available on the system (cached).
///
/// This function caches the result to avoid repeated filesystem lookups
/// and to ensure the warning message is only printed once.
///
/// # Returns
///
/// `true` if lsof is available, `false` otherwise.
fn is_lsof_available() -> bool {
    *LSOF_AVAILABLE.get_or_init(|| {
        let available = which::which("lsof").is_ok();
        if !available {
            eprintln!("Warning: lsof not found, skipping open files display");
        }
        available
    })
}

// lsof output field indices (0-indexed).
// Standard lsof -p output format:
// COMMAND  PID  USER  FD  TYPE  DEVICE  SIZE/OFF  NODE  NAME
// 0        1    2     3   4     5       6         7     8+
//
// Note: NAME (index 8+) may contain spaces, so we join all remaining fields.
const LSOF_FIELD_FD: usize = 3;
const LSOF_FIELD_TYPE: usize = 4;
const LSOF_FIELD_NODE: usize = 7;
const LSOF_FIELD_NAME_START: usize = 8;
const LSOF_MIN_FIELDS: usize = 9;

/// Gets open files for a process using lsof.
///
/// # Arguments
///
/// * `pid` - The process ID to query
///
/// # Returns
///
/// A vector of open files, or `None` if lsof is unavailable or the command fails.
///
/// # Design Note
///
/// This function intentionally returns `Option` rather than `Result` because both
/// failure cases (lsof not installed, lsof failed for specific PID) should result
/// in silently skipping the open files display. The `is_lsof_available()` function
/// already warns users once when lsof is not found. Per-PID failures (e.g., process
/// exited between listing and lsof call, or permission denied) are expected in
/// normal operation and don't warrant additional error messages.
fn get_open_files(pid: u32) -> Option<Vec<OpenFile>> {
    if !is_lsof_available() {
        return None;
    }

    let output = Command::new("lsof")
        .args(["-p", &pid.to_string()])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();

    // Skip the header line and parse each subsequent line
    for line in stdout.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= LSOF_MIN_FIELDS {
            files.push(OpenFile {
                fd: fields[LSOF_FIELD_FD].to_string(),
                file_type: fields[LSOF_FIELD_TYPE].to_string(),
                // NAME field may contain spaces, so join all remaining fields
                name: fields[LSOF_FIELD_NAME_START..].join(" "),
            });
        }
    }

    Some(files)
}

/// Substrings that mark a credential wherever they appear in an uppercased name.
///
/// Each one is long enough that an ordinary name does not hold it by accident.
/// `AUTH` is absent on purpose, because `AUTHOR` and `SSH_AUTH_SOCK` hold it and
/// neither is a secret.
const CREDENTIAL_SUBSTRINGS: &[&str] = &[
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "PASSPHRASE",
    "APIKEY",
    "CREDENTIAL",
    "AUTHORIZATION",
    "PRIVATE_KEY",
    "SIGNING_KEY",
    "SIGNATURE",
    "COOKIE",
    "SESSION_ID",
    "ACCESS_KEY",
];

/// Underscore-separated segments that mark a credential when a whole segment
/// equals one of them.
///
/// A substring test on these would hit `KEYBOARD_LAYOUT` and `PWD`, so the whole
/// segment must match.
const CREDENTIAL_SEGMENTS: &[&str] = &["KEY", "PASS", "PW", "TOKEN", "SECRET"];

/// The text that stands in for a hidden value.
const REDACTED_PLACEHOLDER: &str = "<redacted>";

/// Decides whether an environment variable name looks like it holds a credential.
///
/// The dump of an environment goes to a terminal, and a terminal scrolls into a
/// paste. So a value under a name that reads like a secret is hidden by default.
///
/// # Arguments
///
/// * `name` - The environment variable name
///
/// # Returns
///
/// `true` when the name looks like it holds a credential.
fn looks_like_credential(name: &str) -> bool {
    let upper = name.to_uppercase();
    if CREDENTIAL_SUBSTRINGS
        .iter()
        .any(|marker| upper.contains(marker))
    {
        return true;
    }
    upper
        .split('_')
        .any(|segment| CREDENTIAL_SEGMENTS.contains(&segment))
}

/// Finds the environment block inside a `KERN_PROCARGS2` buffer.
///
/// The kernel lays the buffer out as a 32-bit `argc`, the saved executable path,
/// a run of NUL bytes that pads to an alignment, `argc` argument strings, the
/// environment strings, another run of NUL bytes, and last the `apple[]` strings
/// that `dyld` reads. A probe of a live process on macOS 15 confirmed each part.
/// The environment thus ends at the first empty entry after the arguments, and
/// the `apple[]` strings stay out of the result.
///
/// # Arguments
///
/// * `buffer` - The bytes that `sysctl(KERN_PROCARGS2)` wrote
///
/// # Returns
///
/// The environment block, or `None` when the buffer is too short to hold the
/// parts the layout demands.
#[cfg(target_os = "macos")]
fn env_block_from_procargs2(buffer: &[u8]) -> Option<&[u8]> {
    let argc_bytes: [u8; 4] = buffer.get(0..4)?.try_into().ok()?;
    let argc = i32::from_ne_bytes(argc_bytes);
    let argc = usize::try_from(argc).ok()?;

    let mut cursor = 4;
    // Step over the saved executable path.
    cursor += buffer.get(cursor..)?.iter().position(|byte| *byte == 0)? + 1;
    // Step over the alignment padding that follows it.
    cursor += buffer
        .get(cursor..)?
        .iter()
        .position(|byte| *byte != 0)
        .unwrap_or(0);

    for _ in 0..argc {
        cursor += buffer.get(cursor..)?.iter().position(|byte| *byte == 0)? + 1;
    }

    let start = cursor;
    // The environment ends at the first empty entry, which is where the
    // `apple[]` strings begin.
    while let Some(byte) = buffer.get(cursor) {
        if *byte == 0 {
            break;
        }
        cursor += buffer.get(cursor..)?.iter().position(|b| *b == 0)? + 1;
    }
    buffer.get(start..cursor)
}

/// Parses a NUL-separated block of `NAME=VALUE` entries.
///
/// Both platforms deliver the environment of a process in this shape: Linux in
/// `/proc/<pid>/environ`, macOS in the tail of the `KERN_PROCARGS2` buffer.
///
/// # Arguments
///
/// * `block` - The raw bytes of the environment block
///
/// # Returns
///
/// The name and value of each entry, in the order the kernel gave them. A value
/// that is not valid UTF-8 is converted lossily, because a terminal cannot print
/// the raw bytes anyway. An entry that holds no `=` gets an empty value.
fn parse_environ_block(block: &[u8]) -> Vec<(String, String)> {
    block
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let text = String::from_utf8_lossy(entry);
            match text.split_once('=') {
                Some((name, value)) => (name.to_string(), value.to_string()),
                None => (text.into_owned(), String::new()),
            }
        })
        .collect()
}

/// Hides the value of a credential-looking environment variable.
///
/// # Arguments
///
/// * `name` - The environment variable name
/// * `value` - The environment variable value
/// * `show_secrets` - When `true`, every value is printed in full
///
/// # Returns
///
/// The value to print.
fn redact_env_value(name: &str, value: &str, show_secrets: bool) -> String {
    if !show_secrets && looks_like_credential(name) {
        return REDACTED_PLACEHOLDER.to_string();
    }
    value.to_string()
}

/// Decides whether a process matches a name pattern.
///
/// The retired `procinfo` searched with `pgrep -f`, which reads the whole command
/// line. `match_full_command` gives that reach back.
///
/// # Arguments
///
/// * `pattern` - The substring to look for, ignored when `regex` is given
/// * `regex` - The compiled pattern, when the caller asked for `--regex`
/// * `name` - The executable name of the process
/// * `command` - The whole command line of the process
/// * `match_full_command` - Whether to search the command line instead of the name
/// * `case_sensitive` - Whether the substring search respects case
///
/// # Returns
///
/// `true` when the process matches.
///
/// # Note
///
/// `case_sensitive` governs the substring search only. A regular expression
/// carries its own case rule in the `(?i)` inline flag.
fn matches_name_pattern(
    pattern: &str,
    regex: Option<&Regex>,
    name: &str,
    command: &str,
    match_full_command: bool,
    case_sensitive: bool,
) -> bool {
    // A kernel thread carries no command line, so the name is all there is.
    let subject = if match_full_command && !command.is_empty() {
        command
    } else {
        name
    };

    if let Some(re) = regex {
        return re.is_match(subject);
    }
    if case_sensitive {
        return subject.contains(pattern);
    }
    subject.to_lowercase().contains(&pattern.to_lowercase())
}

/// Warns that a matched process belongs to another user.
///
/// A section that comes back empty teaches the user that the process holds
/// nothing, which is worse than a refusal. So the warning goes out before the
/// sections do.
///
/// # Arguments
///
/// * `processes` - The processes that matched
/// * `current_user` - The name of the user who runs this tool
/// * `is_root` - Whether this tool runs as root, which can read every process
///
/// # Returns
///
/// The warning, or `None` when every matched process is readable.
fn permission_warning(
    processes: &[ProcessInfo],
    current_user: &str,
    is_root: bool,
) -> Option<String> {
    if is_root {
        return None;
    }
    let mut owners: Vec<&str> = processes
        .iter()
        .map(|process| process.user.as_str())
        .filter(|user| *user != current_user)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    if owners.is_empty() {
        return None;
    }
    Some(format!(
        "Warning: some matched processes belong to {}, and you are {current_user}. \
         A section below can come back empty or refused. \
         Run spv again with sudo to see all of it.",
        owners.join(", ")
    ))
}

/// A network connection reported by `lsof -i`.
struct NetConnection {
    fd: String,
    family: String,
    protocol: String,
    name: String,
}

/// Parses one line of `lsof -nP -i -a -p <pid>` output.
///
/// The columns are the same as the ones `get_open_files` reads, with two more in
/// play: NODE carries the protocol, and NAME carries the addresses and the state.
///
/// # Arguments
///
/// * `line` - One line of output, without the header
///
/// # Returns
///
/// The connection, or `None` when the line holds too few fields to be one.
fn parse_lsof_net_line(line: &str) -> Option<NetConnection> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < LSOF_MIN_FIELDS {
        return None;
    }
    Some(NetConnection {
        fd: fields[LSOF_FIELD_FD].to_string(),
        family: fields[LSOF_FIELD_TYPE].to_string(),
        protocol: fields[LSOF_FIELD_NODE].to_string(),
        // The NAME field holds the addresses and, for TCP, the state in
        // parentheses, so every remaining field belongs to it.
        name: fields[LSOF_FIELD_NAME_START..].join(" "),
    })
}

/// Prints processes in table format using comfy-table.
///
/// # Arguments
///
/// * `processes` - The processes to display
/// * `include_cwd` - Whether to include the CWD column
fn print_table(processes: &[ProcessInfo], include_cwd: bool) {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic);

    let mut headers = vec!["PID", "NAME", "USER", "CPU%", "MEM", "STATUS", "COMMAND"];
    if include_cwd {
        headers.push("CWD");
    }
    table.set_header(headers);

    for proc in processes {
        let mut row = vec![
            proc.pid.to_string(),
            proc.name.clone(),
            proc.user.clone(),
            format!("{:.1}", proc.cpu_usage),
            format_memory(proc.memory),
            proc.status.clone(),
            truncate_command(&proc.command, 60),
        ];
        if include_cwd {
            row.push(proc.cwd.clone().unwrap_or_default());
        }
        table.add_row(row);
    }

    println!("{table}");
}

/// Prints processes in raw columnar format.
///
/// # Arguments
///
/// * `processes` - The processes to display
/// * `include_cwd` - Whether to include the CWD column
fn print_raw(processes: &[ProcessInfo], include_cwd: bool) {
    // Print header
    if include_cwd {
        println!(
            "{:>8} {:20} {:10} {:>6} {:>10} {:10} {:40} CWD",
            "PID", "NAME", "USER", "CPU%", "MEM", "STATUS", "COMMAND"
        );
    } else {
        println!(
            "{:>8} {:20} {:10} {:>6} {:>10} {:10} COMMAND",
            "PID", "NAME", "USER", "CPU%", "MEM", "STATUS"
        );
    }

    for proc in processes {
        if include_cwd {
            println!(
                "{:>8} {:20} {:10} {:>6.1} {:>10} {:10} {:40} {}",
                proc.pid,
                truncate_str(&proc.name, 20),
                truncate_str(&proc.user, 10),
                proc.cpu_usage,
                format_memory(proc.memory),
                truncate_str(&proc.status, 10),
                truncate_command(&proc.command, 40),
                proc.cwd.as_deref().unwrap_or("")
            );
        } else {
            println!(
                "{:>8} {:20} {:10} {:>6.1} {:>10} {:10} {}",
                proc.pid,
                truncate_str(&proc.name, 20),
                truncate_str(&proc.user, 10),
                proc.cpu_usage,
                format_memory(proc.memory),
                truncate_str(&proc.status, 10),
                truncate_command(&proc.command, 60)
            );
        }
    }
}

/// Prints open files for processes in table format.
///
/// # Arguments
///
/// * `processes` - The processes to show files for
fn print_open_files(processes: &[ProcessInfo]) {
    for proc in processes {
        if let Some(files) = get_open_files(proc.pid) {
            println!("\nOpen files for {} (PID {}):", proc.name, proc.pid);
            let mut table = Table::new();
            table
                .load_preset(UTF8_FULL)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(vec!["FD", "TYPE", "NAME"]);

            for file in files {
                table.add_row(vec![file.fd, file.file_type, file.name]);
            }
            println!("{table}");
        }
    }
}

/// Truncates a string to a maximum length (in characters), adding "..." if truncated.
///
/// This function is UTF-8 safe and will never panic on multi-byte characters.
/// The max_len refers to the number of Unicode characters, not bytes.
///
/// # Arguments
///
/// * `s` - The string to truncate
/// * `max_len` - Maximum length in characters (not bytes)
///
/// # Returns
///
/// The truncated string.
///
/// # Behavior Notes
///
/// When truncation occurs, the output format is `{truncated_content}...` where
/// `truncated_content` has `max_len - 3` characters. This means:
///
/// - If `max_len >= 4`, the output will be at most `max_len` characters
/// - If `max_len < 4` and truncation is needed, the output will be `"..."` (3 characters),
///   which may exceed the requested `max_len`. This is intentional - we always show
///   the ellipsis to indicate truncation occurred, rather than silently truncating
///   to an even shorter/empty string.
fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let truncate_at = max_len.saturating_sub(3);
        let truncated: String = s.chars().take(truncate_at).collect();
        format!("{truncated}...")
    }
}

/// Truncates a command string intelligently.
///
/// # Arguments
///
/// * `cmd` - The command string to truncate
/// * `max_len` - Maximum length
///
/// # Returns
///
/// The truncated command.
fn truncate_command(cmd: &str, max_len: usize) -> String {
    if cmd.is_empty() {
        return "-".to_string();
    }
    truncate_str(cmd, max_len)
}

/// Formats a memory value in bytes as a human-readable string.
///
/// # Arguments
///
/// * `bytes` - Memory size in bytes (from `sysinfo::Process::memory()`)
///
/// # Returns
///
/// A human-readable string like "1.5 GiB" or "256 MiB".
///
/// # Precision Note
///
/// This function converts `u64` to `f64` for the `human_bytes` crate. The `f64` type
/// can exactly represent integers up to 2^53 (~9 PiB). Since no real-world system
/// has 9+ petabytes of RAM, this conversion is lossless for all practical memory values.
/// This is intentionally not a clippy allow since the reasoning should be documented.
fn format_memory(bytes: u64) -> String {
    human_bytes(bytes as f64)
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Parse the pattern
    let pattern = parse_pattern(&args.pattern);

    // Configure refresh kind based on options
    let mut refresh_kind = ProcessRefreshKind::nothing()
        .with_cmd(UpdateKind::Always)
        .with_cpu()
        .with_memory()
        .with_user(UpdateKind::Always);

    if args.cwd {
        refresh_kind = refresh_kind.with_cwd(UpdateKind::Always);
    }

    // Create system and refresh processes
    let mut system = System::new_with_specifics(RefreshKind::nothing());
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    // Collect matching processes
    let match_options = MatchOptions {
        use_regex: args.regex,
        case_sensitive: args.case_sensitive,
        match_full_command: args.full,
    };
    let processes = collect_processes(&system, &pattern, &match_options, args.cwd)?;

    if processes.is_empty() {
        match &pattern {
            PatternType::SinglePid(pid) => {
                eprintln!("No process found with PID {pid}");
            }
            PatternType::MultiplePids(pids) => {
                eprintln!(
                    "No processes found with PIDs {}",
                    pids.iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            PatternType::NamePattern(p) => {
                eprintln!("No processes found matching '{p}'");
            }
        }
        std::process::exit(1);
    }

    // Print output
    if args.raw {
        print_raw(&processes, args.cwd);
    } else {
        print_table(&processes, args.cwd);
    }

    // Print open files if requested
    if args.lsof {
        print_open_files(&processes);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pattern_single_pid() {
        match parse_pattern("12345") {
            PatternType::SinglePid(pid) => assert_eq!(pid, 12345),
            _ => panic!("Expected SinglePid"),
        }
    }

    #[test]
    fn test_parse_pattern_multiple_pids() {
        match parse_pattern("123,456,789") {
            PatternType::MultiplePids(pids) => {
                assert_eq!(pids, vec![123, 456, 789]);
            }
            _ => panic!("Expected MultiplePids"),
        }
    }

    #[test]
    fn test_parse_pattern_name() {
        match parse_pattern("node") {
            PatternType::NamePattern(name) => assert_eq!(name, "node"),
            _ => panic!("Expected NamePattern"),
        }
    }

    #[test]
    fn test_parse_pattern_version_like() {
        // "2.1.17" contains non-digits, so it's a name pattern
        match parse_pattern("2.1.17") {
            PatternType::NamePattern(name) => assert_eq!(name, "2.1.17"),
            _ => panic!("Expected NamePattern for version-like string"),
        }
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 8), "hello...");
        assert_eq!(truncate_str("hi", 2), "hi");
    }

    #[test]
    fn test_truncate_str_utf8_safety() {
        // Multi-byte UTF-8 characters should not cause panics
        // Japanese characters (3 bytes each in UTF-8)
        assert_eq!(truncate_str("日本語", 10), "日本語"); // Under limit (3 chars)
        assert_eq!(truncate_str("日本語テスト", 6), "日本語テスト"); // Exactly at limit (6 chars)
        assert_eq!(truncate_str("日本語テスト", 5), "日本..."); // Truncated (6 chars > 5)

        // Emoji (4 bytes each in UTF-8)
        assert_eq!(truncate_str("🎉🎊🎁", 10), "🎉🎊🎁"); // Under limit (3 chars)
        assert_eq!(truncate_str("🎉🎊🎁🎈🎂", 5), "🎉🎊🎁🎈🎂"); // Exactly at limit (5 chars)
        assert_eq!(truncate_str("🎉🎊🎁🎈🎂", 4), "🎉..."); // Truncated (5 chars > 4)

        // Mixed ASCII and multi-byte
        assert_eq!(truncate_str("café", 10), "café"); // Under limit (4 chars)
        assert_eq!(truncate_str("café au lait", 8), "café ..."); // Truncated (12 chars > 8)

        // Edge case: exactly at limit
        assert_eq!(truncate_str("hello", 5), "hello");

        // Edge case: very small max_len (see test below for detailed documentation)
        assert_eq!(truncate_str("hello world", 3), "...");
        assert_eq!(truncate_str("日本語", 3), "日本語"); // Exactly at limit (3 chars)
        assert_eq!(truncate_str("日本語テ", 3), "..."); // Truncated (4 chars > 3)
    }

    /// Documents the intentional behavior when max_len < 4.
    ///
    /// When truncation is needed but max_len is very small (< 4), the function
    /// will still output "..." to indicate truncation occurred. This means the
    /// output may exceed the requested max_len. This is intentional - we prioritize
    /// indicating that truncation happened over strictly adhering to max_len.
    ///
    /// Users of this function should ensure max_len >= 4 if they need strict
    /// length guarantees when truncation might occur.
    #[test]
    fn test_truncate_str_small_max_len_intentional_behavior() {
        // When max_len < 4 and truncation is needed, output is "..." (3 chars)
        // which exceeds the requested max_len. This is intentional.
        assert_eq!(truncate_str("hello", 2), "..."); // 5 chars > 2, output "..." (3 chars)
        assert_eq!(truncate_str("hello", 1), "..."); // 5 chars > 1, output "..." (3 chars)
        assert_eq!(truncate_str("hello", 0), "..."); // 5 chars > 0, output "..." (3 chars)

        // When no truncation needed, string is returned as-is
        assert_eq!(truncate_str("hi", 2), "hi"); // 2 chars <= 2, no truncation
        assert_eq!(truncate_str("a", 1), "a"); // 1 char <= 1, no truncation

        // max_len = 3 is the minimum where output length equals max_len when truncating
        assert_eq!(truncate_str("hello", 3), "..."); // Exactly 3 chars output
        assert_eq!(truncate_str("hello", 4), "h..."); // 4 chars output (1 char + "...")
    }

    #[test]
    fn test_truncate_command_empty() {
        assert_eq!(truncate_command("", 10), "-");
    }

    #[test]
    fn test_format_memory() {
        // Basic formatting tests (human_bytes uses IEC units: KiB, MiB, GiB, TiB)
        assert_eq!(format_memory(0), "0 B");
        assert_eq!(format_memory(1024), "1 KiB");
        assert_eq!(format_memory(1024 * 1024), "1 MiB");
        assert_eq!(format_memory(1024 * 1024 * 1024), "1 GiB");

        // Verify large values don't panic (precision is documented in the function)
        let one_tib = 1024_u64 * 1024 * 1024 * 1024;
        assert_eq!(format_memory(one_tib), "1 TiB");
    }

    /// Parses a single line of lsof output into an OpenFile struct.
    ///
    /// This is extracted for testing purposes to verify the field indices are correct.
    fn parse_lsof_line(line: &str) -> Option<OpenFile> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= LSOF_MIN_FIELDS {
            Some(OpenFile {
                fd: fields[LSOF_FIELD_FD].to_string(),
                file_type: fields[LSOF_FIELD_TYPE].to_string(),
                name: fields[LSOF_FIELD_NAME_START..].join(" "),
            })
        } else {
            None
        }
    }

    #[test]
    fn test_lsof_parsing_standard_line() {
        // Example lsof output line (simplified for testing)
        // COMMAND  PID  USER  FD   TYPE   DEVICE  SIZE/OFF  NODE  NAME
        let line = "bash     1234 user  cwd  DIR    1,5     4096      2  /home/user";
        let file = parse_lsof_line(line).expect("Should parse valid lsof line");
        assert_eq!(file.fd, "cwd");
        assert_eq!(file.file_type, "DIR");
        assert_eq!(file.name, "/home/user");
    }

    #[test]
    fn test_lsof_parsing_name_with_spaces() {
        // File path containing spaces should be handled correctly
        let line = "bash     1234 user  3r   REG    1,5     1024      3  /home/user/my file.txt";
        let file = parse_lsof_line(line).expect("Should parse line with spaces in name");
        assert_eq!(file.fd, "3r");
        assert_eq!(file.file_type, "REG");
        assert_eq!(file.name, "/home/user/my file.txt");
    }

    #[test]
    fn test_lsof_parsing_insufficient_fields() {
        // Lines with fewer than LSOF_MIN_FIELDS should be skipped
        let line = "bash 1234 user cwd DIR";
        assert!(parse_lsof_line(line).is_none());
    }

    #[test]
    fn test_lsof_field_constants_consistency() {
        // Verify that field constants are consistent with expected lsof format
        // This test documents the expected format and catches accidental changes
        assert_eq!(LSOF_FIELD_FD, 3, "FD should be at index 3");
        assert_eq!(LSOF_FIELD_TYPE, 4, "TYPE should be at index 4");
        assert_eq!(LSOF_FIELD_NAME_START, 8, "NAME should start at index 8");
        assert_eq!(LSOF_MIN_FIELDS, 9, "Minimum fields should be 9");
    }

    #[test]
    fn credential_names_are_recognized() {
        for name in [
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "DB_PASSWORD",
            "API_KEY",
            "npm_config_password",
            "SESSION_COOKIE",
            "JWT_SIGNATURE",
            "MY_PASSPHRASE",
            "AUTHORIZATION",
            "SERVICE_CREDENTIALS",
            "DEPLOY_PW",
            "OLD_PASSWD",
        ] {
            assert!(
                looks_like_credential(name),
                "{name} names a credential and must be hidden"
            );
        }
    }

    #[test]
    fn ordinary_names_are_not_credentials() {
        for name in [
            "PWD",
            "AUTHOR",
            "SSH_AUTH_SOCK",
            "KEYBOARD_LAYOUT",
            "MONKEY_BUSINESS",
            "HOME",
            "PATH",
            "LANG",
            "TERM",
            "SHELL",
            "USER",
            "HOMEBREW_NO_ANALYTICS",
            "PASSENGER_ROOT",
        ] {
            assert!(
                !looks_like_credential(name),
                "{name} is an ordinary name and must stay visible"
            );
        }
    }

    #[test]
    fn a_credential_value_is_hidden_by_default() {
        assert_eq!(
            redact_env_value("GITHUB_TOKEN", "ghp_notarealtoken", false),
            "<redacted>"
        );
    }

    #[test]
    fn show_secrets_prints_a_credential_value_in_full() {
        assert_eq!(
            redact_env_value("GITHUB_TOKEN", "ghp_notarealtoken", true),
            "ghp_notarealtoken"
        );
    }

    #[test]
    fn an_ordinary_value_stays_visible() {
        assert_eq!(redact_env_value("HOME", "/Users/tim", false), "/Users/tim");
        assert_eq!(
            redact_env_value("GREETING", "こんにちは", false),
            "こんにちは"
        );
    }

    #[test]
    fn an_environ_block_becomes_name_and_value_pairs() {
        let block = b"HOME=/root\0PATH=/bin:/usr/bin\0";
        assert_eq!(
            parse_environ_block(block),
            vec![
                ("HOME".to_string(), "/root".to_string()),
                ("PATH".to_string(), "/bin:/usr/bin".to_string()),
            ]
        );
    }

    #[test]
    fn an_environ_value_keeps_its_own_equals_signs() {
        assert_eq!(
            parse_environ_block(b"OPTS=a=b=c\0"),
            vec![("OPTS".to_string(), "a=b=c".to_string())]
        );
    }

    #[test]
    fn an_environ_entry_without_an_equals_sign_gets_an_empty_value() {
        assert_eq!(
            parse_environ_block(b"WEIRD\0"),
            vec![("WEIRD".to_string(), String::new())]
        );
    }

    #[test]
    fn an_environ_block_carries_multi_byte_characters_through() {
        assert_eq!(
            parse_environ_block("GREETING=こんにちは\0EMOJI=🎉\0".as_bytes()),
            vec![
                ("GREETING".to_string(), "こんにちは".to_string()),
                ("EMOJI".to_string(), "🎉".to_string()),
            ]
        );
    }

    #[test]
    fn an_empty_environ_block_holds_no_entries() {
        assert!(parse_environ_block(b"").is_empty());
        assert!(parse_environ_block(b"\0\0").is_empty());
    }

    /// Builds a buffer with the layout that `sysctl(KERN_PROCARGS2)` writes.
    #[cfg(target_os = "macos")]
    fn procargs2_fixture(
        argc: i32,
        exec_path: &str,
        argv: &[&str],
        envp: &[&str],
        apple: &[&str],
    ) -> Vec<u8> {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&argc.to_ne_bytes());
        buffer.extend_from_slice(exec_path.as_bytes());
        // The saved path terminator, then the alignment padding the kernel adds.
        buffer.extend_from_slice(&[0, 0, 0, 0, 0]);
        for entry in argv {
            buffer.extend_from_slice(entry.as_bytes());
            buffer.push(0);
        }
        for entry in envp {
            buffer.extend_from_slice(entry.as_bytes());
            buffer.push(0);
        }
        // The run of NUL bytes that closes the environment.
        buffer.extend_from_slice(&[0, 0, 0]);
        for entry in apple {
            buffer.extend_from_slice(entry.as_bytes());
            buffer.push(0);
        }
        buffer
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn procargs2_yields_the_environment_without_the_apple_strings() {
        let buffer = procargs2_fixture(
            2,
            "/usr/bin/tool",
            &["/usr/bin/tool", "--flag"],
            &["HOME=/root", "GREETING=こんにちは"],
            &["executable_path=/usr/bin/tool", "ptr_munge=0x1"],
        );
        let block = env_block_from_procargs2(&buffer).expect("the fixture holds a whole layout");
        assert_eq!(
            parse_environ_block(block),
            vec![
                ("HOME".to_string(), "/root".to_string()),
                ("GREETING".to_string(), "こんにちは".to_string()),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn procargs2_yields_an_empty_block_for_a_process_with_no_environment() {
        let buffer = procargs2_fixture(1, "/usr/bin/tool", &["/usr/bin/tool"], &[], &["pfz=0x2"]);
        let block = env_block_from_procargs2(&buffer).expect("the fixture holds a whole layout");
        assert!(parse_environ_block(block).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_truncated_procargs2_buffer_yields_nothing() {
        assert!(env_block_from_procargs2(&[]).is_none());
        assert!(env_block_from_procargs2(&[1, 0, 0]).is_none());

        // A buffer that stops in the middle of the saved executable path. An
        // argument count that disagrees with the entries present is NOT a case
        // this function can catch: trailing NUL bytes read exactly like the
        // empty arguments that `prog "" ""` really produces.
        let mut buffer =
            procargs2_fixture(2, "/usr/bin/tool", &["/usr/bin/tool", "--flag"], &[], &[]);
        buffer.truncate(9);
        assert!(env_block_from_procargs2(&buffer).is_none());

        // A buffer that stops in the middle of an argument.
        let mut buffer =
            procargs2_fixture(2, "/usr/bin/tool", &["/usr/bin/tool", "--flag"], &[], &[]);
        buffer.truncate(26);
        assert!(env_block_from_procargs2(&buffer).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_negative_argument_count_yields_nothing() {
        let buffer = procargs2_fixture(-1, "/usr/bin/tool", &["/usr/bin/tool"], &[], &[]);
        assert!(env_block_from_procargs2(&buffer).is_none());
    }

    #[test]
    fn an_established_connection_line_parses() {
        // Captured from `lsof -nP -i` on macOS 15.
        let line = "2.1.258 7323 timmattison   12u  IPv4 0xa466e292ab820022      0t0  TCP 192.168.0.128:61932->160.79.104.10:443 (ESTABLISHED)";
        let connection = parse_lsof_net_line(line).expect("this line names a connection");
        assert_eq!(connection.fd, "12u");
        assert_eq!(connection.family, "IPv4");
        assert_eq!(connection.protocol, "TCP");
        assert_eq!(
            connection.name,
            "192.168.0.128:61932->160.79.104.10:443 (ESTABLISHED)"
        );
    }

    #[test]
    fn a_listening_socket_line_parses() {
        let line = "nginx  512 root    6u  IPv4 0x1234567890abcdef      0t0  TCP *:8080 (LISTEN)";
        let connection = parse_lsof_net_line(line).expect("this line names a listening socket");
        assert_eq!(connection.protocol, "TCP");
        assert_eq!(connection.name, "*:8080 (LISTEN)");
    }

    #[test]
    fn a_datagram_socket_line_has_no_state() {
        let line = "mDNSRespo  200 nobody   9u  IPv6 0xfedcba0987654321      0t0  UDP *:5353";
        let connection = parse_lsof_net_line(line).expect("this line names a datagram socket");
        assert_eq!(connection.protocol, "UDP");
        assert_eq!(connection.family, "IPv6");
        assert_eq!(connection.name, "*:5353");
    }

    #[test]
    fn a_line_with_too_few_fields_is_not_a_connection() {
        assert!(parse_lsof_net_line("nginx 512 root 6u IPv4").is_none());
    }

    /// Builds a process whose only interesting field is its owner.
    fn process_owned_by(pid: u32, user: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: "tool".to_string(),
            user: user.to_string(),
            cpu_usage: 0.0,
            memory: 0,
            status: "Sleep".to_string(),
            command: "tool".to_string(),
            cwd: None,
        }
    }

    #[test]
    fn your_own_processes_raise_no_warning() {
        let processes = [process_owned_by(1, "tim"), process_owned_by(2, "tim")];
        assert!(permission_warning(&processes, "tim", false).is_none());
    }

    #[test]
    fn another_users_process_raises_a_warning_that_names_both_users() {
        let processes = [process_owned_by(1, "root"), process_owned_by(2, "tim")];
        let warning =
            permission_warning(&processes, "tim", false).expect("root is not tim, so warn");
        assert!(warning.contains("root"), "the warning names the owner: {warning}");
        assert!(warning.contains("tim"), "the warning names the caller: {warning}");
        assert!(warning.contains("sudo"), "the warning names the remedy: {warning}");
    }

    #[test]
    fn root_reads_every_process_and_raises_no_warning() {
        let processes = [process_owned_by(1, "root"), process_owned_by(2, "nobody")];
        assert!(permission_warning(&processes, "root", true).is_none());
    }

    #[test]
    fn a_warning_names_each_other_owner_once_and_in_order() {
        let processes = [
            process_owned_by(1, "root"),
            process_owned_by(2, "nobody"),
            process_owned_by(3, "root"),
            process_owned_by(4, "tim"),
        ];
        let warning = permission_warning(&processes, "tim", false).expect("two owners differ");
        assert!(
            warning.contains("nobody, root"),
            "the owners come once each and in order: {warning}"
        );
    }

    #[test]
    fn a_substring_search_ignores_case_by_default() {
        assert!(matches_name_pattern(
            "NODE", None, "node", "node app.js", false, false
        ));
    }

    #[test]
    fn case_sensitive_makes_a_substring_search_exact() {
        assert!(!matches_name_pattern(
            "NODE", None, "node", "node app.js", false, true
        ));
        assert!(matches_name_pattern(
            "node", None, "node", "node app.js", false, true
        ));
    }

    #[test]
    fn the_full_command_line_reaches_past_the_executable_name() {
        assert!(!matches_name_pattern(
            "deploy",
            None,
            "zsh",
            "zsh -c deploy.sh",
            false,
            false
        ));
        assert!(matches_name_pattern(
            "deploy",
            None,
            "zsh",
            "zsh -c deploy.sh",
            true,
            false
        ));
    }

    #[test]
    fn the_full_command_line_falls_back_to_the_name_when_it_is_empty() {
        // A kernel thread carries no command line.
        assert!(matches_name_pattern(
            "kernel",
            None,
            "kernel_task",
            "",
            true,
            false
        ));
    }

    #[test]
    fn a_regex_runs_against_the_subject_the_flags_chose() {
        let re = Regex::new("deploy.*sh").expect("this pattern compiles");
        assert!(!matches_name_pattern(
            "",
            Some(&re),
            "zsh",
            "zsh -c deploy.sh",
            false,
            false
        ));
        assert!(matches_name_pattern(
            "",
            Some(&re),
            "zsh",
            "zsh -c deploy.sh",
            true,
            false
        ));
    }

    #[test]
    fn no_section_flag_prints_no_section() {
        assert_eq!(
            Args::parse_from(["spv", "zsh"]).sections(),
            Sections {
                cwd: false,
                files: false,
                env: false,
                net: false
            }
        );
    }

    #[test]
    fn all_turns_on_every_section() {
        assert_eq!(
            Args::parse_from(["spv", "--all", "zsh"]).sections(),
            Sections {
                cwd: true,
                files: true,
                env: true,
                net: true
            }
        );
    }

    #[test]
    fn a_single_section_flag_turns_on_only_that_section() {
        assert_eq!(
            Args::parse_from(["spv", "--env", "zsh"]).sections(),
            Sections {
                cwd: false,
                files: false,
                env: true,
                net: false
            }
        );
    }

    #[test]
    fn section_flags_combine() {
        assert_eq!(
            Args::parse_from(["spv", "--cwd", "--net", "zsh"]).sections(),
            Sections {
                cwd: true,
                files: false,
                env: false,
                net: true
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_status_code_to_string_all_branches() {
        assert_eq!(status_code_to_string(1), "Idle");
        assert_eq!(status_code_to_string(2), "Run");
        assert_eq!(status_code_to_string(3), "Sleep");
        assert_eq!(status_code_to_string(4), "Stop");
        assert_eq!(status_code_to_string(5), "Zombie");
        assert_eq!(status_code_to_string(0), "Unknown(0)");
        assert_eq!(status_code_to_string(6), "Unknown(6)");
        assert_eq!(status_code_to_string(255), "Unknown(255)");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_kinfo_proc_size_matches_kernel() {
        // Validate that our hardcoded SIZE constant matches the actual kernel struct.
        // sysctl returns data in multiples of kinfo_proc, so the returned size must
        // be evenly divisible by our constant.
        use kinfo_proc_layout::SIZE;

        let result = get_all_process_info_via_sysctl();
        assert!(
            result.is_some(),
            "sysctl(KERN_PROC_ALL) should succeed on macOS"
        );

        let map = result.unwrap();
        // PID 1 (launchd) must always exist and be owned by root (UID 0)
        let pid1 = map.get(&1);
        assert!(pid1.is_some(), "PID 1 (launchd) must be present");
        assert_eq!(pid1.unwrap().uid, 0, "PID 1 should be owned by root");

        // Verify the size constant is correct by checking struct alignment:
        // the sysctl data size must be divisible by our SIZE constant
        assert_eq!(SIZE, 648, "kinfo_proc size constant must be 648 bytes");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_sysctl_returns_current_process() {
        let result = get_all_process_info_via_sysctl();
        assert!(result.is_some());

        let map = result.unwrap();
        let my_pid = std::process::id();
        assert!(
            map.contains_key(&my_pid),
            "sysctl should return info for our own process (PID {my_pid})"
        );
    }
}
