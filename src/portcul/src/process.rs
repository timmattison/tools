use std::fmt;

/// A process ID, wrapping the raw `u32` from the `listeners` crate.
///
/// Prevents accidentally mixing up PIDs with other `u32` values (port numbers,
/// array indices, etc.) at the type level.
///
/// Construction validates that the value fits in `i32` (the POSIX `pid_t` type),
/// preventing `kill(-1, sig)` which would signal every process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pid(u32);

/// Error returned when a PID value exceeds the valid range for POSIX `pid_t`.
#[derive(Debug, Clone)]
pub struct PidRangeError(u32);

impl fmt::Display for PidRangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PID {} exceeds maximum valid value {} (would wrap to negative in POSIX kill())",
            self.0,
            i32::MAX
        )
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
        if value > i32::MAX as u32 {
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
#[derive(Debug, Clone)]
pub struct ListeningProcess {
    /// Process ID.
    pub pid: Pid,
    /// Process name.
    pub name: String,
    /// Port number.
    pub port: u16,
    /// Bind address (e.g., "0.0.0.0", "127.0.0.1", "::1").
    pub address: String,
}

/// Collects all processes currently listening on ports.
///
/// Uses the `listeners` crate which wraps platform-specific APIs
/// (lsof on macOS, /proc on Linux) to discover listening sockets.
///
/// # Errors
///
/// Returns an error if the underlying system call to enumerate listeners fails.
pub fn collect_listeners() -> anyhow::Result<Vec<ListeningProcess>> {
    let raw_listeners = listeners::get_all()
        .map_err(|e| anyhow::anyhow!("failed to enumerate listeners: {e}"))?;

    let mut result: Vec<ListeningProcess> = raw_listeners
        .iter()
        .map(|listener| {
            Ok(ListeningProcess {
                pid: Pid::try_from(listener.process.pid)?,
                name: listener.process.name.clone(),
                port: listener.socket.port(),
                address: listener.socket.ip().to_string(),
            })
        })
        .collect::<Result<Vec<_>, PidRangeError>>()
        .map_err(|e| anyhow::anyhow!("{e}"))?;

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
/// Returns an error if the kill syscall fails (e.g., insufficient permissions).
pub fn kill_process(pid: Pid) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: libc::kill is a standard POSIX call. We pass a valid signal number
        // (SIGTERM = 15). Pid::as_i32() is safe because the Pid type validates at
        // construction that the value fits in i32, preventing kill(-1, sig).
        let ret = unsafe { libc::kill(pid.as_i32(), libc::SIGTERM) };
        if ret != 0 {
            // Fall back to /bin/kill when libc::kill returns EPERM
            // (some macOS processes require the external kill binary)
            let errno = std::io::Error::last_os_error();
            if errno.raw_os_error() == Some(libc::EPERM) {
                let status = std::process::Command::new("/bin/kill")
                    .arg("-TERM")
                    .arg(pid.as_u32().to_string())
                    .status()?;
                if !status.success() {
                    return Err(anyhow::anyhow!(
                        "failed to kill PID {pid}: permission denied"
                    ));
                }
                return Ok(());
            }
            return Err(anyhow::anyhow!("failed to kill PID {pid}: {errno}"));
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

/// Formats a listening process as a single line for CLI output.
///
/// Output format: `  PID <pid>  <name>   <address>:<port>`
pub fn format_process_line(listener: &ListeningProcess) -> String {
    format!(
        "  PID {:<7} {:<20} {}:{}",
        listener.pid, listener.name, listener.address, listener.port
    )
}

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
    fn test_filter_by_port_returns_matching() {
        let listeners = vec![
            ListeningProcess {
                pid: Pid::try_from(100_u32).unwrap(),
                name: "nginx".to_string(),
                port: 80,
                address: "0.0.0.0".to_string(),
            },
            ListeningProcess {
                pid: Pid::try_from(200_u32).unwrap(),
                name: "node".to_string(),
                port: 3000,
                address: "127.0.0.1".to_string(),
            },
            ListeningProcess {
                pid: Pid::try_from(300_u32).unwrap(),
                name: "nginx".to_string(),
                port: 80,
                address: "0.0.0.0".to_string(),
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
            address: "0.0.0.0".to_string(),
        };
        let line = format_process_line(&listener);
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
            address: "0.0.0.0".to_string(),
        };
        let b = ListeningProcess {
            pid: Pid::try_from(99999_u32).unwrap(),
            name: "long-process-name".to_string(),
            port: 443,
            address: "127.0.0.1".to_string(),
        };
        // Both lines should be well-formed (no panic, contains expected data)
        let line_a = format_process_line(&a);
        let line_b = format_process_line(&b);
        assert!(line_a.contains("PID"));
        assert!(line_b.contains("PID"));
    }

    #[test]
    fn test_filter_by_port_returns_empty_when_no_match() {
        let listeners = vec![ListeningProcess {
            pid: Pid::try_from(100_u32).unwrap(),
            name: "nginx".to_string(),
            port: 80,
            address: "0.0.0.0".to_string(),
        }];
        let filtered = filter_by_port(&listeners, 9999);
        assert!(filtered.is_empty());
    }
}
