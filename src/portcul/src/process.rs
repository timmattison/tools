use std::fmt;

/// A process ID, wrapping the raw `u32` from the `listeners` crate.
///
/// Prevents accidentally mixing up PIDs with other `u32` values (port numbers,
/// array indices, etc.) at the type level.
///
/// Construction validates that the value is in `1..=i32::MAX` (the positive
/// range of the POSIX `pid_t` type). This refuses `kill(0, sig)`, which signals
/// the process group of the caller, and `kill(-1, sig)`, which signals every
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pid(u32);

/// Error returned when a PID value is outside `1..=i32::MAX`.
#[derive(Debug, Clone)]
pub struct PidRangeError(u32);

impl fmt::Display for PidRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            write!(
                f,
                "PID 0 is not one process (POSIX kill() would signal the whole process group)"
            )
        } else {
            write!(
                f,
                "PID {} exceeds maximum valid value {} (would wrap to negative in POSIX kill())",
                self.0,
                i32::MAX
            )
        }
    }
}

impl std::error::Error for PidRangeError {}

impl Pid {
    /// Returns the raw `u32` value.
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// Returns the value as `i32`, safe because construction validated the range.
    pub fn as_i32(self) -> i32 {
        // Construction guarantees self.0 <= i32::MAX, so this never wraps.
        self.0.cast_signed()
    }
}

impl TryFrom<u32> for Pid {
    type Error = PidRangeError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        if value == 0 || value > i32::MAX as u32 {
            return Err(PidRangeError(value));
        }
        Ok(Self(value))
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A process listening on a port.
///
/// A single `ListeningProcess` represents all sockets that a given process
/// has open on a single port. A process can listen on multiple addresses
/// for the same port (e.g., `0.0.0.0` and `::` for dual-stack); those are
/// collected into `addresses` in sorted order.
#[derive(Debug, Clone)]
pub struct ListeningProcess {
    /// Process ID.
    pub pid: Pid,
    /// Process name, with each control character escaped by [`escape_controls`].
    pub name: String,
    /// Port number.
    pub port: u16,
    /// Bind addresses for this (pid, port), sorted. Non-empty.
    pub addresses: Vec<String>,
}

/// Groups flat `ListeningProcess` entries by (port, pid, name), merging their
/// addresses into a single entry with a sorted, deduplicated address list.
///
/// Inputs with any number of addresses are accepted; all of them flow into the
/// output group. The result order tracks the first occurrence of each group
/// key in the input.
pub fn group_by_process(flat: Vec<ListeningProcess>) -> Vec<ListeningProcess> {
    use std::collections::HashMap;

    // Key by (port, pid, name). Track insertion order via a parallel Vec so
    // the output is deterministic given a deterministic input order.
    let mut index: HashMap<(u16, Pid, String), usize> = HashMap::new();
    let mut groups: Vec<ListeningProcess> = Vec::new();

    for entry in flat {
        let key = (entry.port, entry.pid, entry.name.clone());
        match index.get(&key) {
            Some(&i) => groups[i].addresses.extend(entry.addresses),
            None => {
                index.insert(key, groups.len());
                groups.push(entry);
            }
        }
    }

    for group in &mut groups {
        group.addresses.sort();
        group.addresses.dedup();
    }

    groups
}

/// Makes a process name safe to print on a terminal.
///
/// A process name comes from outside the program. A control character in it,
/// for example ESC or a newline, would reach the terminal whole, so an escape
/// sequence could change the screen and a newline could fake a second row.
/// Each control character becomes its `escape_default` text (for example
/// `\u{1b}` or `\n`), as `ps` escapes such bytes. Each other character stays.
pub fn escape_controls(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() {
            escaped.extend(c.escape_default());
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// Collects all processes currently listening on ports.
///
/// Uses the `listeners` crate, which reads the kernel directly to find the
/// listening sockets: libproc through FFI on macOS, and `/proc` on Linux. It
/// starts no external program.
///
/// # Errors
///
/// Returns an error if the underlying system call to enumerate listeners fails.
pub fn collect_listeners() -> anyhow::Result<Vec<ListeningProcess>> {
    let raw_listeners =
        listeners::get_all().map_err(|e| anyhow::anyhow!("failed to enumerate listeners: {e}"))?;

    let flat: Vec<ListeningProcess> = raw_listeners
        .iter()
        .map(|listener| {
            Ok(ListeningProcess {
                pid: Pid::try_from(listener.process.pid)?,
                name: escape_controls(&listener.process.name),
                port: listener.socket.port(),
                addresses: vec![listener.socket.ip().to_string()],
            })
        })
        .collect::<Result<Vec<_>, PidRangeError>>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let mut result = group_by_process(flat);
    // Sort by port number, then by PID for stable ordering
    result.sort_by(|a, b| a.port.cmp(&b.port).then(a.pid.cmp(&b.pid)));
    Ok(result)
}

/// Sends a signal to kill a process by PID.
///
/// Uses SIGTERM (15) for a graceful shutdown request.
///
/// # Errors
///
/// Returns the error of the kill syscall, with its errno (for example EPERM,
/// when the process belongs to a different user). ESRCH gets its own message,
/// because it means that the process exited after the list was read.
pub fn kill_process(pid: Pid) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill is a standard POSIX call. We pass a valid signal number
        // (SIGTERM = 15). Pid::as_i32() is safe because the Pid type validates at
        // construction that the value is in 1..=i32::MAX, preventing kill(0, sig)
        // and kill(-1, sig).
        let ret = unsafe { libc::kill(pid.as_i32(), libc::SIGTERM) };
        if ret != 0 {
            return Err(kill_error(&std::io::Error::last_os_error()));
        }
        Ok(())
    }

    #[cfg(not(unix))]
    {
        Err(anyhow::anyhow!(
            "killing processes is not supported on this platform"
        ))
    }
}

/// Sends SIGTERM to each target through `kill`, and prints one line for each
/// result.
///
/// A success prints to standard output, and a failure prints to standard
/// error. The function tries each target, also after a failure.
///
/// # Errors
///
/// Returns an error that counts the failed kills when one kill or more failed.
pub fn kill_targets(
    targets: &[&ListeningProcess],
    kill: impl Fn(Pid) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let mut failed = 0_usize;
    for listener in targets {
        match kill(listener.pid) {
            Ok(()) => println!("{}", format_kill_result(listener)),
            Err(e) => {
                eprintln!("Failed to kill PID {}: {e}", listener.pid);
                failed += 1;
            }
        }
    }
    let total = targets.len();
    anyhow::ensure!(
        failed == 0,
        "{failed} of {total} {} failed",
        if total == 1 { "kill" } else { "kills" }
    );
    Ok(())
}

/// Turns the OS error of a failed kill(2) into the error of [`kill_process`].
///
/// The text of `error` stays in the message, so the errno is not lost. ESRCH
/// gets its own message, because it means that the process exited after the
/// list was read.
#[cfg(unix)]
fn kill_error(error: &std::io::Error) -> anyhow::Error {
    if error.raw_os_error() == Some(libc::ESRCH) {
        anyhow::anyhow!("the process exited before the signal reached it ({error})")
    } else {
        anyhow::anyhow!("{error}")
    }
}

/// Formats the header shown before listing processes to kill.
///
/// Example: "Found 2 processes on port 8080:"
pub fn format_kill_header(count: usize, port: u16) -> String {
    let noun = if count == 1 { "process" } else { "processes" };
    format!("Found {count} {noun} on port {port}:")
}

/// Formats the result message after sending SIGTERM.
///
/// Example: "Sent SIGTERM to nginx (PID 1234)"
pub fn format_kill_result(listener: &ListeningProcess) -> String {
    format!("Sent SIGTERM to {} (PID {})", listener.name, listener.pid)
}

/// Formats a single (process, address) pair as a line for CLI output.
///
/// Output format: `  PID <pid>  <name>   <address>:<port>`
///
/// The name is padded to [`NAME_COLUMN_WIDTH`] display columns, so a wide
/// name (CJK, emoji) does not move the address column.
pub fn format_process_line(listener: &ListeningProcess, address: &str) -> String {
    format!(
        "  PID {:<7} {} {}:{}",
        listener.pid,
        textfit::pad_right(&listener.name, NAME_COLUMN_WIDTH),
        address,
        listener.port
    )
}

/// The display width of the name column of [`format_process_line`].
const NAME_COLUMN_WIDTH: usize = 20;

/// Filters listeners to only those on the given port.
pub fn filter_by_port(listeners: &[ListeningProcess], port: u16) -> Vec<&ListeningProcess> {
    listeners.iter().filter(|l| l.port == port).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_listeners_returns_sorted() {
        // This is an integration test - it calls the real system
        // We can't assert specific processes, but we can verify the sort order
        if let Ok(listeners) = collect_listeners() {
            for window in listeners.windows(2) {
                assert!(
                    (window[0].port, window[0].pid) <= (window[1].port, window[1].pid),
                    "listeners should be sorted by (port, pid)"
                );
            }
        }
    }

    #[test]
    fn test_kill_nonexistent_process() {
        // Use a very high PID that almost certainly doesn't exist.
        // PID 0 must NOT be used because kill(0, sig) sends to the entire process group.
        let result = kill_process(Pid::try_from(4_000_000).unwrap());
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_kill_of_an_exited_process_says_that_it_exited() {
        // No process has this PID, so kill(2) gives ESRCH: the process
        // exited after the list was read.
        let error = kill_process(Pid::try_from(4_000_000_u32).unwrap()).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("exited"), "ESRCH needs its own message: {text}");
        assert!(text.contains("os error"), "the message must keep the errno: {text}");
    }

    fn kill_target(pid: u32) -> ListeningProcess {
        ListeningProcess {
            pid: Pid::try_from(pid).unwrap(),
            name: "server".to_string(),
            port: 8080,
            addresses: vec!["0.0.0.0".to_string()],
        }
    }

    #[test]
    fn test_kill_targets_fails_when_every_kill_fails() {
        let a = kill_target(100);
        let b = kill_target(200);
        let result = kill_targets(&[&a, &b], |_| Err(anyhow::anyhow!("EPERM")));
        let error = result.expect_err("a failed kill must give an error");
        assert!(error.to_string().contains("2 of 2"), "{error}");
    }

    #[test]
    fn test_kill_targets_fails_when_one_kill_fails() {
        let a = kill_target(100);
        let b = kill_target(200);
        let bad = Pid::try_from(200_u32).unwrap();
        let tried = std::cell::RefCell::new(Vec::new());
        let result = kill_targets(&[&a, &b], |pid| {
            tried.borrow_mut().push(pid);
            if pid == bad {
                Err(anyhow::anyhow!("EPERM"))
            } else {
                Ok(())
            }
        });
        let error = result.expect_err("a failed kill must give an error");
        assert!(error.to_string().contains("1 of 2"), "{error}");
        assert_eq!(tried.borrow().len(), 2, "each target gets a kill");
    }

    #[test]
    fn test_kill_targets_succeeds_when_every_kill_succeeds() {
        let a = kill_target(100);
        assert!(kill_targets(&[&a], |_| Ok(())).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn test_kill_error_keeps_the_errno_of_eperm() {
        let text = kill_error(&std::io::Error::from_raw_os_error(libc::EPERM)).to_string();
        let expected = std::io::Error::from_raw_os_error(libc::EPERM).to_string();
        assert_eq!(text, expected);
        assert!(!text.contains("exited"), "only ESRCH says exited: {text}");
    }

    #[test]
    fn test_pid_display() {
        assert_eq!(Pid::try_from(1234_u32).unwrap().to_string(), "1234");
    }

    #[test]
    fn test_pid_ordering() {
        let pid1 = Pid::try_from(1_u32).unwrap();
        let pid2 = Pid::try_from(2_u32).unwrap();
        assert!(pid1 < pid2);
        assert_eq!(
            Pid::try_from(42_u32).unwrap(),
            Pid::try_from(42_u32).unwrap()
        );
    }

    #[test]
    fn test_pid_rejects_values_above_i32_max() {
        // u32::MAX would wrap to -1 as i32, causing kill(-1, sig) to signal all processes
        assert!(Pid::try_from(u32::MAX).is_err());
        assert!(Pid::try_from(i32::MAX as u32 + 1).is_err());
    }

    #[test]
    fn test_pid_rejects_zero() {
        // kill(0, sig) signals the whole process group of the caller, which
        // holds portcul itself.
        let error = Pid::try_from(0_u32).expect_err("PID 0 is not one process");
        assert!(error.to_string().contains("process group"), "{error}");
    }

    #[test]
    fn test_pid_accepts_i32_max() {
        let pid = Pid::try_from(i32::MAX as u32).unwrap();
        assert_eq!(pid.as_i32(), i32::MAX);
        assert_eq!(pid.as_u32(), i32::MAX as u32);
    }

    #[test]
    fn test_pid_as_i32_matches_as_u32() {
        let pid = Pid::try_from(1234_u32).unwrap();
        assert_eq!(pid.as_i32(), 1234);
        assert_eq!(pid.as_u32(), 1234);
    }

    #[test]
    fn test_format_kill_header_single() {
        let header = format_kill_header(1, 8080);
        assert_eq!(header, "Found 1 process on port 8080:");
    }

    #[test]
    fn test_format_kill_header_multiple() {
        let header = format_kill_header(3, 443);
        assert_eq!(header, "Found 3 processes on port 443:");
    }

    #[test]
    fn test_format_kill_result() {
        let listener = ListeningProcess {
            pid: Pid::try_from(1234_u32).unwrap(),
            name: "nginx".to_string(),
            port: 8080,
            addresses: vec!["0.0.0.0".to_string()],
        };
        let msg = format_kill_result(&listener);
        assert_eq!(msg, "Sent SIGTERM to nginx (PID 1234)");
    }

    #[test]
    fn test_filter_by_port_returns_matching() {
        let listeners = vec![
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "nginx".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(200_u32).unwrap(),
                name: "node".to_string(),
                port: 3000,
                addresses: vec!["127.0.0.1".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(300_u32).unwrap(),
                name: "nginx".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
        ];
        let filtered = filter_by_port(&listeners, 80);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].pid, Pid::try_from(100_u32).unwrap());
        assert_eq!(filtered[1].pid, Pid::try_from(300_u32).unwrap());
    }

    #[test]
    fn test_format_process_line_basic() {
        let listener = ListeningProcess {
            pid: Pid::try_from(1234_u32).unwrap(),
            name: "nginx".to_string(),
            port: 8080,
            addresses: vec!["0.0.0.0".to_string()],
        };
        let line = format_process_line(&listener, "0.0.0.0");
        assert!(line.contains("1234"), "should contain PID");
        assert!(line.contains("nginx"), "should contain process name");
        assert!(line.contains("0.0.0.0:8080"), "should contain address:port");
    }

    #[test]
    fn test_format_process_line_alignment() {
        let a = ListeningProcess {
            pid: Pid::try_from(1_u32).unwrap(),
            name: "a".to_string(),
            port: 80,
            addresses: vec!["0.0.0.0".to_string()],
        };
        let b = ListeningProcess {
            pid: Pid::try_from(99999_u32).unwrap(),
            name: "long-process-name".to_string(),
            port: 443,
            addresses: vec!["127.0.0.1".to_string()],
        };
        // Both lines should be well-formed (no panic, contains expected data)
        let line_a = format_process_line(&a, "0.0.0.0");
        let line_b = format_process_line(&b, "127.0.0.1");
        assert!(line_a.contains("PID"));
        assert!(line_b.contains("PID"));
    }

    #[test]
    fn test_escape_controls_removes_each_control_character() {
        let escaped = escape_controls("evil\u{1b}[2J\nname\u{7f}");
        assert!(
            !escaped.chars().any(char::is_control),
            "no control character may reach the terminal: {escaped:?}"
        );
        assert_eq!(escaped, "evil\\u{1b}[2J\\nname\\u{7f}");
    }

    #[test]
    fn test_escape_controls_keeps_multibyte_names() {
        for name in ["日本語", "🎉", "café", "cafe\u{301}"] {
            assert_eq!(escape_controls(name), name);
        }
    }

    #[test]
    fn test_format_process_line_pads_names_by_display_width() {
        // The address column must start at the same terminal column for each
        // name, so the padding counts display columns, not characters.
        let column = |name: &str| {
            let listener = ListeningProcess {
                pid: Pid::try_from(1_u32).unwrap(),
                name: name.to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            };
            let line = format_process_line(&listener, "0.0.0.0");
            let (head, _) = line.rsplit_once(" 0.0.0.0:80").unwrap();
            unicode_width::UnicodeWidthStr::width(head)
        };
        let expected = column("nginx");
        for name in ["日本語", "🎉🎊", "café", "cafe\u{301}"] {
            assert_eq!(column(name), expected, "the column after {name:?} moved");
        }
    }

    #[test]
    fn test_filter_by_port_returns_empty_when_no_match() {
        let listeners = vec![ListeningProcess {
            pid: Pid::try_from(100_u32).unwrap(),
            name: "nginx".to_string(),
            port: 80,
            addresses: vec!["0.0.0.0".to_string()],
        }];
        let filtered = filter_by_port(&listeners, 9999);
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_group_by_process_merges_addresses() {
        let flat = vec![
            ListeningProcess {
                pid: Pid::try_from(1221_u32).unwrap(),
                name: "ControlCenter".to_string(),
                port: 5000,
                addresses: vec!["::".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(1221_u32).unwrap(),
                name: "ControlCenter".to_string(),
                port: 5000,
                addresses: vec!["0.0.0.0".to_string()],
            },
        ];
        let grouped = group_by_process(flat);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].pid, Pid::try_from(1221_u32).unwrap());
        assert_eq!(grouped[0].port, 5000);
        // Addresses should be sorted for stable ordering
        assert_eq!(grouped[0].addresses, vec!["0.0.0.0", "::"]);
    }

    #[test]
    fn test_group_by_process_keeps_distinct_processes_separate() {
        let flat = vec![
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "a".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(200_u32).unwrap(),
                name: "b".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
        ];
        let grouped = group_by_process(flat);
        // Different PIDs on same port must remain separate
        assert_eq!(grouped.len(), 2);
    }

    #[test]
    fn test_group_by_process_different_ports_stay_separate() {
        let flat = vec![
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "a".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "a".to_string(),
                port: 443,
                addresses: vec!["0.0.0.0".to_string()],
            },
        ];
        let grouped = group_by_process(flat);
        // Same PID, different ports must remain separate
        assert_eq!(grouped.len(), 2);
    }

    #[test]
    fn test_group_by_process_deduplicates_addresses() {
        let flat = vec![
            ListeningProcess {
                pid: Pid::try_from(1_u32).unwrap(),
                name: "x".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
            ListeningProcess {
                pid: Pid::try_from(1_u32).unwrap(),
                name: "x".to_string(),
                port: 80,
                addresses: vec!["0.0.0.0".to_string()],
            },
        ];
        let grouped = group_by_process(flat);
        assert_eq!(grouped.len(), 1);
        // Duplicate address entries should not produce duplicate addresses
        assert_eq!(grouped[0].addresses, vec!["0.0.0.0"]);
    }
}
