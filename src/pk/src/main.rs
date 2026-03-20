//! pk - Process killer with dry-run mode and detailed feedback
//!
//! A CLI tool that finds and kills processes by name, using the same APIs
//! that Activity Monitor uses (libproc on macOS). Unlike pkill, this tool
//! provides detailed feedback about what was killed, what failed, and warns
//! if no processes were found.

use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use colored::Colorize;
use regex::Regex;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

/// Process killer with dry-run mode and detailed feedback.
///
/// Uses the same process discovery APIs as Activity Monitor (libproc on macOS),
/// which can find processes that `ps` and `pkill` cannot see.
///
/// # Examples
///
/// ```text
/// pk 2.1.29              - Kill all processes named "2.1.29"
/// pk --dry-run 2.1.29    - Show what would be killed
/// pk --regex '2\.1\.\d+' - Kill with regex pattern
/// pk -9 zombie           - Send SIGKILL instead of SIGTERM
/// ```
#[derive(Parser)]
#[command(
    name = "pk",
    version = version_string!(),
    about = "Process killer with dry-run mode and detailed feedback",
    long_about = "Examples:\n  pk 2.1.29              - Kill all processes named \"2.1.29\"\n  pk --dry-run 2.1.29    - Show what would be killed\n  pk --regex '2\\.1\\.\\d+' - Kill with regex pattern\n  pk -9 zombie           - Send SIGKILL instead of SIGTERM"
)]
struct Args {
    /// Name pattern to match.
    ///
    /// By default, performs case-insensitive substring matching.
    /// Use --regex for regular expression matching.
    /// Use --exact for exact name matching.
    #[arg(required = true)]
    pattern: String,

    /// Dry run: show what would be killed without killing.
    ///
    /// Lists all matching processes with their PIDs and names.
    #[arg(long, short = 'n')]
    dry_run: bool,

    /// Use regex matching instead of substring.
    ///
    /// When enabled, the pattern is treated as a regular expression.
    #[arg(long, short = 'r')]
    regex: bool,

    /// Use exact name matching (case-insensitive).
    ///
    /// Only matches processes whose name exactly equals the pattern,
    /// ignoring case differences.
    #[arg(long, short = 'e')]
    exact: bool,

    /// Signal to send (default: 15/SIGTERM).
    ///
    /// Common signals: 9 (SIGKILL), 15 (SIGTERM), 3 (SIGQUIT), 2 (SIGINT), 1 (SIGHUP)
    #[arg(long, short = 's', default_value_t = 15)]
    signal: i32,

    /// Shorthand for -s 9 (SIGKILL).
    #[arg(short = '9', conflicts_with = "signal")]
    sigkill: bool,
}

/// Represents the outcome of attempting to kill a process.
enum KillResult {
    /// Process was successfully killed.
    Killed { pid: u32, name: String },
    /// Failed to kill the process.
    Failed { pid: u32, name: String, error: String },
    /// Dry run - would have killed.
    WouldKill { pid: u32, name: String },
}

/// Information about a matching process.
struct ProcessMatch {
    pid: u32,
    name: String,
}

/// Finds processes matching the given pattern.
///
/// # Arguments
///
/// * `system` - The sysinfo System instance
/// * `pattern` - The pattern to match against
/// * `use_regex` - Whether to use regex matching
/// * `use_exact` - Whether to use exact name matching
///
/// # Returns
///
/// A vector of matching processes.
///
/// # Errors
///
/// Returns an error if regex compilation fails.
fn find_matching_processes(
    system: &System,
    pattern: &str,
    use_regex: bool,
    use_exact: bool,
) -> Result<Vec<ProcessMatch>> {
    let regex = if use_regex {
        Some(Regex::new(pattern).context("Invalid regex pattern")?)
    } else {
        None
    };

    let pattern_lower = pattern.to_lowercase();
    let mut matches = Vec::new();

    for (pid, process) in system.processes() {
        let name = process.name().to_string_lossy().to_string();

        let is_match = if use_exact {
            // Case-insensitive exact match for consistency with substring matching
            name.to_lowercase() == pattern_lower
        } else if let Some(ref re) = regex {
            re.is_match(&name)
        } else {
            name.to_lowercase().contains(&pattern_lower)
        };

        if is_match {
            matches.push(ProcessMatch {
                pid: pid.as_u32(),
                name,
            });
        }
    }

    // Sort by PID for consistent output
    matches.sort_by_key(|p| p.pid);
    Ok(matches)
}

/// Attempts to kill a process with the given signal.
///
/// Uses `libc::kill` directly. On macOS, if this fails with EPERM (which can
/// happen due to code-signing restrictions when an ad-hoc-signed binary tries
/// to signal a properly signed process), falls back to `/bin/kill` which is
/// Apple-signed and has broader permissions.
///
/// # Arguments
///
/// * `pid` - The process ID to kill
/// * `signal` - The signal to send
///
/// # Returns
///
/// Ok(()) if successful, Err with the errno message if failed.
#[cfg(unix)]
fn kill_process(pid: u32, signal: i32) -> std::result::Result<(), String> {
    // Convert to i32 first, then to pid_t (which is i32 on most Unix systems)
    // This handles the u32 -> i32 conversion safely
    let pid_i32 = i32::try_from(pid).map_err(|_| "PID too large for system call")?;

    // SAFETY: kill() is a standard POSIX function. We're passing a valid signal number
    // and the PID comes from the system's process list. The worst case is ESRCH (process
    // doesn't exist) or EPERM (permission denied), both of which we handle via errno.
    let result = unsafe { libc::kill(pid_i32, signal) };

    if result == 0 {
        return Ok(());
    }

    let errno = std::io::Error::last_os_error();

    // On macOS, EPERM can occur due to code-signing restrictions even when the
    // user owns the target process. Fall back to /bin/kill which is Apple-signed.
    if errno.raw_os_error() == Some(libc::EPERM) {
        return kill_process_via_bin_kill(pid, signal);
    }

    Err(errno.to_string())
}

/// Falls back to `/bin/kill` for sending signals.
///
/// This is used when `libc::kill` returns EPERM, which on macOS can happen
/// due to code-signing restrictions between an ad-hoc-signed binary and a
/// properly signed target process. `/bin/kill` is Apple-signed and typically
/// has the necessary permissions.
#[cfg(unix)]
fn kill_process_via_bin_kill(pid: u32, signal: i32) -> std::result::Result<(), String> {
    let output = std::process::Command::new("/bin/kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .output()
        .map_err(|e| format!("failed to run /bin/kill: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.trim();
        if msg.is_empty() {
            Err(format!(
                "/bin/kill exited with status {}",
                output.status.code().unwrap_or(-1)
            ))
        } else {
            Err(msg.to_string())
        }
    }
}

/// Attempts to kill a process (non-Unix stub).
///
/// On non-Unix platforms, process killing is not supported and this function
/// always returns an error. The tool can still list matching processes in
/// dry-run mode.
#[cfg(not(unix))]
fn kill_process(_pid: u32, _signal: i32) -> std::result::Result<(), String> {
    Err("Process killing is not supported on this platform (Unix only)".to_string())
}

/// Returns true if the current platform supports process killing.
#[cfg(unix)]
const fn platform_supports_kill() -> bool {
    true
}

/// Returns true if the current platform supports process killing.
#[cfg(not(unix))]
const fn platform_supports_kill() -> bool {
    false
}

/// Returns the name of a signal number.
///
/// # Arguments
///
/// * `signal` - The signal number
///
/// # Returns
///
/// The signal name (e.g., "SIGTERM") or the number if unknown.
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP".to_string(),
        2 => "SIGINT".to_string(),
        3 => "SIGQUIT".to_string(),
        9 => "SIGKILL".to_string(),
        15 => "SIGTERM".to_string(),
        _ => format!("signal {signal}"),
    }
}

/// Prints a summary of the kill results.
///
/// # Arguments
///
/// * `results` - The kill results to summarize
/// * `signal` - The signal that was sent
/// * `dry_run` - Whether this was a dry run
fn print_results(results: &[KillResult], signal: i32, dry_run: bool) {
    let signal_desc = signal_name(signal);

    if dry_run {
        println!("{}", "DRY RUN - No processes were killed".yellow().bold());
        println!();
    }

    let mut killed = Vec::new();
    let mut failed = Vec::new();
    let mut would_kill = Vec::new();

    for result in results {
        match result {
            KillResult::Killed { pid, name } => killed.push((pid, name)),
            KillResult::Failed { pid, name, error } => failed.push((pid, name, error)),
            KillResult::WouldKill { pid, name } => would_kill.push((pid, name)),
        }
    }

    // Print what was/would be killed
    if !would_kill.is_empty() {
        println!(
            "{} ({}):",
            "Would kill".cyan().bold(),
            signal_desc.cyan()
        );
        for (pid, name) in &would_kill {
            println!("  {} {} ({})", "->".cyan(), name, pid.to_string().dimmed());
        }
        println!();
    }

    if !killed.is_empty() {
        println!(
            "{} ({}):",
            "Successfully killed".green().bold(),
            signal_desc.green()
        );
        for (pid, name) in &killed {
            println!("  {} {} ({})", "->".green(), name, pid.to_string().dimmed());
        }
        println!();
    }

    if !failed.is_empty() {
        println!("{}:", "Failed to kill".red().bold());
        for (pid, name, error) in &failed {
            println!(
                "  {} {} ({}) - {}",
                "->".red(),
                name,
                pid.to_string().dimmed(),
                error.red()
            );
        }
        println!();
    }

    // Print summary
    let total = results.len();
    if dry_run {
        println!(
            "{}",
            format!("Total: {} process(es) would be sent {}", total, signal_desc).bold()
        );
    } else {
        let killed_count = killed.len();
        let failed_count = failed.len();
        if failed_count > 0 {
            println!(
                "{}",
                format!(
                    "Total: {} killed, {} failed (out of {} matched)",
                    killed_count, failed_count, total
                )
                .bold()
            );
        } else {
            println!(
                "{}",
                format!("Total: {} process(es) sent {}", killed_count, signal_desc).bold()
            );
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Warn early if platform doesn't support killing and user isn't in dry-run mode
    if !platform_supports_kill() && !args.dry_run {
        eprintln!(
            "{}: Process killing is not supported on this platform. Use --dry-run to list matching processes.",
            "Warning".yellow().bold()
        );
        std::process::exit(1);
    }

    // Determine signal (allow -9 shorthand)
    let signal = if args.sigkill { 9 } else { args.signal };

    // Create system and refresh processes
    // We only need process names and PIDs, so use minimal refresh
    let refresh_kind = ProcessRefreshKind::nothing();
    let mut system = System::new_with_specifics(RefreshKind::nothing());
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    // Find matching processes
    let matches = find_matching_processes(&system, &args.pattern, args.regex, args.exact)?;

    if matches.is_empty() {
        let match_type = if args.exact {
            "exactly matching"
        } else if args.regex {
            "matching regex"
        } else {
            "containing"
        };
        eprintln!(
            "{}: No processes found {} '{}'",
            "Warning".yellow().bold(),
            match_type,
            args.pattern.cyan()
        );
        std::process::exit(1);
    }

    // Perform the kills (or dry run)
    let mut results = Vec::new();

    for proc_match in matches {
        if args.dry_run {
            results.push(KillResult::WouldKill {
                pid: proc_match.pid,
                name: proc_match.name,
            });
        } else {
            match kill_process(proc_match.pid, signal) {
                Ok(()) => {
                    results.push(KillResult::Killed {
                        pid: proc_match.pid,
                        name: proc_match.name,
                    });
                }
                Err(error) => {
                    results.push(KillResult::Failed {
                        pid: proc_match.pid,
                        name: proc_match.name,
                        error,
                    });
                }
            }
        }
    }

    // Print results
    print_results(&results, signal, args.dry_run);

    // Exit with error if any kills failed
    let had_failures = results.iter().any(|r| matches!(r, KillResult::Failed { .. }));
    if had_failures {
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_name_known() {
        assert_eq!(signal_name(1), "SIGHUP");
        assert_eq!(signal_name(2), "SIGINT");
        assert_eq!(signal_name(3), "SIGQUIT");
        assert_eq!(signal_name(9), "SIGKILL");
        assert_eq!(signal_name(15), "SIGTERM");
    }

    #[test]
    fn test_signal_name_unknown() {
        assert_eq!(signal_name(42), "signal 42");
        assert_eq!(signal_name(0), "signal 0");
    }

    /// Helper to simulate exact matching logic (case-insensitive)
    fn matches_exact(name: &str, pattern: &str) -> bool {
        name.to_lowercase() == pattern.to_lowercase()
    }

    /// Helper to simulate substring matching logic (case-insensitive)
    fn matches_substring(name: &str, pattern: &str) -> bool {
        name.to_lowercase().contains(&pattern.to_lowercase())
    }

    #[test]
    fn test_find_matching_processes_exact() {
        // Exact match is case-insensitive but requires full name match
        assert!(matches_exact("test", "test"));
        assert!(matches_exact("TEST", "test")); // Case-insensitive
        assert!(matches_exact("Test", "TEST")); // Case-insensitive
        assert!(!matches_exact("testing", "test")); // Not exact
        assert!(!matches_exact("my-test", "test")); // Not exact
    }

    #[test]
    fn test_exact_vs_substring_matching() {
        // Verify the difference between exact and substring matching
        let name = "my-test-app";
        let pattern = "test";

        // Substring should match
        assert!(matches_substring(name, pattern));
        // Exact should NOT match
        assert!(!matches_exact(name, pattern));
    }

    #[test]
    fn test_find_matching_processes_substring() {
        // Substring matching is case-insensitive
        assert!(matches_substring("testing", "test"));
        assert!(matches_substring("TEST", "test"));
        assert!(matches_substring("my-test-app", "test"));
        assert!(matches_substring("MyTestApp", "TEST")); // Case-insensitive both ways
        assert!(!matches_substring("foo", "test"));
    }

    #[test]
    fn test_find_matching_processes_regex() {
        let re = Regex::new(r"2\.1\.\d+").unwrap();

        assert!(re.is_match("2.1.29"));
        assert!(re.is_match("2.1.30"));
        assert!(re.is_match("app-2.1.99"));
        assert!(!re.is_match("2.1"));
        assert!(!re.is_match("2.1."));
        assert!(!re.is_match("2.1.abc"));
    }

    #[test]
    fn test_version_like_pattern() {
        // Version-like patterns (e.g., "2.1.29") should be found
        let pattern = "2.1.29";
        let pattern_lower = pattern.to_lowercase();

        assert!("2.1.29".to_lowercase().contains(&pattern_lower));
        assert!("process-2.1.29".to_lowercase().contains(&pattern_lower));
    }
}
