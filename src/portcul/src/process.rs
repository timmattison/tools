use std::net::SocketAddr;

/// A process listening on a port.
#[derive(Debug, Clone)]
pub struct ListeningProcess {
    /// Process ID.
    pub pid: u32,
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

    let mut result = Vec::new();

    for listener in &raw_listeners {
        let socket_str = format!("{}", listener.socket);

        let (port, address) = if let Ok(addr) = socket_str.parse::<SocketAddr>() {
            (addr.port(), addr.ip().to_string())
        } else {
            // Fallback: try to extract port from "addr:port" format
            match socket_str.rsplit_once(':') {
                Some((addr, port_str)) => match port_str.parse::<u16>() {
                    Ok(port) => (port, addr.to_string()),
                    Err(_) => continue,
                },
                None => continue,
            }
        };

        result.push(ListeningProcess {
            pid: listener.process.pid,
            name: listener.process.name.clone(),
            port,
            address,
        });
    }

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
pub fn kill_process(pid: u32) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "PID values from the OS fit in i32 on all supported platforms"
        )]
        // SAFETY: libc::kill is a standard POSIX call. We pass a valid signal number
        // (SIGTERM = 15). The PID cast is safe for any u32 value on 64-bit systems.
        let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if ret != 0 {
            // Fall back to /bin/kill when libc::kill returns EPERM
            // (some macOS processes require the external kill binary)
            let errno = std::io::Error::last_os_error();
            if errno.raw_os_error() == Some(libc::EPERM) {
                let status = std::process::Command::new("/bin/kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
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
        let result = kill_process(4_000_000);
        assert!(result.is_err());
    }
}
