//! sp - Smart process viewer with enhanced filtering and display
//!
//! A CLI tool that provides enhanced process listing with flexible filtering
//! and display options.

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
/// sp 77763           - Show process with PID 77763
/// sp 77763,82313     - Show multiple PIDs
/// sp node            - Find processes containing 'node'
/// sp --regex 'node.*' - Find with regex
/// sp --cwd zsh       - Show processes with their working directories
/// sp --lsof $$       - Show open files for current shell
/// ```
#[derive(Parser)]
#[command(
    name = "sp",
    version = version_string!(),
    about = "Smart process viewer with enhanced filtering and display",
    long_about = "Examples:\n  sp 77763           - Show process with PID 77763\n  sp 77763,82313     - Show multiple PIDs\n  sp node            - Find processes containing 'node'\n  sp --regex 'node.*' - Find with regex\n  sp --cwd zsh       - Show processes with their CWD\n  sp --lsof $$       - Show open files for process"
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

    /// Raw output without table formatting.
    ///
    /// Produces columnar output similar to traditional ps.
    #[arg(long)]
    raw: bool,
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
        CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    }
}

/// Collects process information based on the pattern.
///
/// # Arguments
///
/// * `system` - The sysinfo System instance
/// * `pattern` - The parsed pattern type
/// * `use_regex` - Whether to use regex matching for name patterns
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
    use_regex: bool,
    include_cwd: bool,
) -> Result<Vec<ProcessInfo>> {
    let mut processes = Vec::new();
    let regex = match pattern {
        PatternType::NamePattern(p) if use_regex => {
            Some(Regex::new(p).context("Invalid regex pattern")?)
        }
        _ => None,
    };

    for (pid, process) in system.processes() {
        let pid_u32 = pid.as_u32();
        let name = process.name().to_string_lossy().to_string();

        let matches = match pattern {
            PatternType::SinglePid(p) => pid_u32 == *p,
            PatternType::MultiplePids(pids) => pids.contains(&pid_u32),
            PatternType::NamePattern(p) => {
                if let Some(ref re) = regex {
                    re.is_match(&name)
                } else {
                    name.to_lowercase().contains(&p.to_lowercase())
                }
            }
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
                .unwrap_or_else(|| "unknown".to_string());

            let status = format!("{:?}", process.status());

            let command = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ");

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
    let processes = collect_processes(&system, &pattern, args.regex, args.cwd)?;

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
        let one_tib = 1024u64 * 1024 * 1024 * 1024;
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
}
