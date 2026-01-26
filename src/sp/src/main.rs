//! sp - Smart process viewer with enhanced filtering and display
//!
//! A CLI tool that provides enhanced process listing with flexible filtering
//! and display options.

use std::ffi::CStr;
use std::process::Command;

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use comfy_table::{presets::UTF8_FULL, ContentArrangement, Table};
use human_bytes::human_bytes;
use regex::Regex;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

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

#[cfg(not(unix))]
fn get_username(uid: u32) -> String {
    uid.to_string()
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
                    // uid.to_string() returns "Uid(123)" format, we need the raw number
                    let uid_str = uid.to_string();
                    // Parse out the number from "Uid(123)" or just use it as uid
                    if let Some(start) = uid_str.find('(') {
                        if let Some(end) = uid_str.find(')') {
                            if let Ok(n) = uid_str[start + 1..end].parse::<u32>() {
                                return get_username(n);
                            }
                        }
                    }
                    // Try direct parse as number
                    if let Ok(n) = uid_str.parse::<u32>() {
                        return get_username(n);
                    }
                    uid_str
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

/// Gets open files for a process using lsof.
///
/// # Arguments
///
/// * `pid` - The process ID to query
///
/// # Returns
///
/// A vector of open files, or None if lsof is unavailable.
fn get_open_files(pid: u32) -> Option<Vec<OpenFile>> {
    // Check if lsof is available
    if which::which("lsof").is_err() {
        eprintln!("Warning: lsof not found, skipping open files display");
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

    for line in stdout.lines().skip(1) {
        // Skip header
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 9 {
            files.push(OpenFile {
                fd: fields[3].to_string(),
                file_type: fields[4].to_string(),
                name: fields[8..].join(" "), // File path may contain spaces
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
            human_bytes(proc.memory as f64),
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
                human_bytes(proc.memory as f64),
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
                human_bytes(proc.memory as f64),
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

/// Truncates a string to a maximum length, adding "..." if truncated.
///
/// # Arguments
///
/// * `s` - The string to truncate
/// * `max_len` - Maximum length
///
/// # Returns
///
/// The truncated string.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
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
    fn test_truncate_command_empty() {
        assert_eq!(truncate_command("", 10), "-");
    }
}
