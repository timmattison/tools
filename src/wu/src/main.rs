use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use walkdir::WalkDir;

#[derive(Parser)]
#[command(
    name = "wu",
    version = version_string!(),
    about = "Cross-platform tool to identify which processes have a file, directory, or device open",
    long_about = "wu (who's using) identifies which processes currently have a file, directory, or device open. When given a directory, it recursively checks all files within. Works on macOS, Linux, and Windows."
)]
struct Args {
    /// Paths to check for open handles (files or directories)
    #[arg(required = true, num_args = 1..)]
    paths: Vec<PathBuf>,

    /// Output format as JSON
    #[arg(long, short)]
    json: bool,

    /// Verbose output with additional details
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub user: Option<String>,
    pub access_mode: Option<String>,
    pub file_descriptor: Option<String>,
    pub file_path: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut all_processes = Vec::new();

    for path in &args.paths {
        let processes = who_is_using(path)
            .with_context(|| format!("Failed to check processes using path: {}", path.display()))?;
        all_processes.extend(processes);
    }

    // Remove duplicates while preserving order
    let mut seen = HashSet::new();
    let mut unique_processes = Vec::new();
    for process in all_processes {
        if seen.insert((process.pid, process.file_path.clone())) {
            unique_processes.push(process);
        }
    }

    if unique_processes.is_empty() {
        if args.json {
            println!("[]");
        } else {
            let paths_str = args
                .paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("No processes found using: {}", paths_str);
        }
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&unique_processes)?);
    } else {
        print_human_readable(&unique_processes, args.verbose);
    }

    Ok(())
}

fn print_human_readable(processes: &[ProcessInfo], verbose: bool) {
    println!("Processes using the specified paths:");
    println!();

    if verbose {
        // Group by PID for verbose output
        let mut by_pid: HashMap<u32, Vec<&ProcessInfo>> = HashMap::new();
        for process in processes {
            by_pid.entry(process.pid).or_default().push(process);
        }

        for (pid, procs) in by_pid {
            let first = procs[0];
            println!("PID: {}", pid);
            println!("Name: {}", first.name);
            if let Some(user) = &first.user {
                println!("User: {}", user);
            }
            println!("Files:");
            for proc in procs {
                if let Some(path) = &proc.file_path {
                    println!(
                        "  - {} ({})",
                        path.display(),
                        proc.access_mode.as_deref().unwrap_or("unknown")
                    );
                    if let Some(fd) = &proc.file_descriptor {
                        println!("    File Descriptor: {}", fd);
                    }
                }
            }
            println!();
        }
    } else {
        println!(
            "{:<8} {:<20} {:<15} {:<10} FILE",
            "PID", "NAME", "USER", "ACCESS"
        );
        println!("{}", "-".repeat(80));

        for process in processes {
            let file_str = process
                .file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "unknown".to_string());

            println!(
                "{:<8} {:<20} {:<15} {:<10} {}",
                process.pid,
                truncate_string(&process.name, 20),
                process.user.as_deref().unwrap_or("unknown"),
                process.access_mode.as_deref().unwrap_or("unknown"),
                truncate_string(&file_str, 40)
            );
        }
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    // Compare on character counts (not bytes) and truncate via chars().take so
    // we never slice in the middle of a multi-byte UTF-8 codepoint.
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let keep = max_len.saturating_sub(3);
        let prefix: String = s.chars().take(keep).collect();
        format!("{prefix}...")
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn collect_files_recursively(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            files.push(entry.path().to_path_buf());
        }
    } else {
        // It might be a device or special file
        files.push(path.to_path_buf());
    }

    Ok(files)
}

/// The directory where Linux shows one entry for each process.
#[cfg(target_os = "linux")]
const PROC_ROOT: &str = "/proc";

#[cfg(target_os = "linux")]
fn who_is_using(path: &Path) -> Result<Vec<ProcessInfo>> {
    get_file_users_linux(path, Path::new(PROC_ROOT))
}

#[cfg(target_os = "macos")]
fn who_is_using(path: &Path) -> Result<Vec<ProcessInfo>> {
    get_file_users_macos(path)
}

#[cfg(target_os = "windows")]
fn who_is_using(path: &Path) -> Result<Vec<ProcessInfo>> {
    get_file_users_windows(path)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn who_is_using(_path: &Path) -> Result<Vec<ProcessInfo>> {
    anyhow::bail!("Unsupported platform");
}

/// Finds each process that holds `target_path`, or a file under it, open.
///
/// `target_path` is a file or a directory. `proc_root` is the directory that
/// holds one entry for each process. It is `/proc` in production and a fake
/// directory in a test. The function returns an error when it cannot read
/// `proc_root` itself or cannot walk `target_path`.
#[cfg(target_os = "linux")]
fn get_file_users_linux(target_path: &Path, proc_root: &Path) -> Result<Vec<ProcessInfo>> {
    use procfs::process::{all_processes_with_root, FDTarget, MMapPath};
    use std::collections::HashSet;

    let files = collect_files_recursively(target_path)?;
    let mut canonical_files = HashSet::new();

    for file in &files {
        if let Ok(canonical) = file.canonicalize() {
            canonical_files.insert(canonical);
        } else {
            canonical_files.insert(file.clone());
        }
    }

    let mut processes = Vec::new();

    for process in all_processes_with_root(proc_root)? {
        let process = process?;
        // The kernel gives each process a positive id.
        let Ok(pid) = u32::try_from(process.pid) else {
            continue;
        };

        // Get process info
        let stat = match process.stat() {
            Ok(stat) => stat,
            Err(_) => continue, // Process might have died
        };

        let status = match process.status() {
            Ok(status) => status,
            Err(_) => continue,
        };

        // Check file descriptors. procfs reads the target of each descriptor
        // link. A socket, a pipe, or an anonymous inode has no path to match.
        if let Ok(fds) = process.fd() {
            for fd_info in fds.flatten() {
                let FDTarget::Path(fd_path) = fd_info.target else {
                    continue;
                };
                let fd_canonical = fd_path.canonicalize().unwrap_or_else(|_| fd_path.clone());

                // Check if this fd matches any of our target files
                for target_file in &canonical_files {
                    if fd_canonical == *target_file
                        || target_file.starts_with(&fd_canonical)
                        || fd_canonical.starts_with(target_file)
                    {
                        processes.push(ProcessInfo {
                            pid,
                            name: stat.comm.clone(),
                            user: Some(status.ruid.to_string()),
                            access_mode: Some(format!("fd:{}", fd_info.fd)),
                            file_descriptor: Some(fd_info.fd.to_string()),
                            file_path: Some(fd_path.clone()),
                        });
                        break;
                    }
                }
            }
        }

        // Check memory maps. Only a map of a file has a path to match.
        if let Ok(maps) = process.maps() {
            for map in maps {
                let MMapPath::Path(map_path) = map.pathname else {
                    continue;
                };
                let map_canonical = map_path.canonicalize().unwrap_or_else(|_| map_path.clone());

                // Check if this mmap matches any of our target files
                for target_file in &canonical_files {
                    if map_canonical == *target_file
                        || target_file.starts_with(&map_canonical)
                        || map_canonical.starts_with(target_file)
                    {
                        processes.push(ProcessInfo {
                            pid,
                            name: stat.comm.clone(),
                            user: Some(status.ruid.to_string()),
                            access_mode: Some("mmap".to_string()),
                            file_descriptor: None,
                            file_path: Some(map_path.clone()),
                        });
                        break;
                    }
                }
            }
        }
    }

    Ok(processes)
}

#[cfg(target_os = "macos")]
fn get_file_users_macos(target_path: &Path) -> Result<Vec<ProcessInfo>> {
    use std::process::Command;

    // Check if lsof is available
    if which::which("lsof").is_err() {
        anyhow::bail!("lsof command not found. Please install lsof to use this tool on macOS.");
    }

    let mut all_processes = Vec::new();

    // Use +D for directories (recursive) and regular path for files
    let is_dir = target_path.is_dir();
    let target_str = target_path.to_string_lossy();

    let output = if is_dir {
        // Use +D for recursive directory search
        Command::new("lsof")
            .args(["+D", &target_str])
            .output()
            .context("Failed to execute lsof command")?
    } else {
        // Use regular lsof for files
        Command::new("lsof")
            .arg(target_str.as_ref())
            .output()
            .context("Failed to execute lsof command")?
    };

    if output.stdout.is_empty() {
        // No processes found
        return Ok(Vec::new());
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    all_processes.extend(parse_lsof_detailed_output(&output_str)?);

    Ok(all_processes)
}

#[cfg(target_os = "macos")]
fn parse_lsof_detailed_output(output: &str) -> Result<Vec<ProcessInfo>> {
    let mut processes = Vec::new();

    for line in output.lines().skip(1) {
        // Skip header
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 9 {
            let name = fields[0].to_string();
            let pid = fields[1].parse::<u32>().context("Failed to parse PID")?;
            let user = Some(fields[2].to_string());
            let fd = fields[3].to_string();
            let access_mode = Some(fields[4].to_string());
            let file_path = Some(PathBuf::from(fields[8..].join(" "))); // File path may contain spaces

            processes.push(ProcessInfo {
                pid,
                name,
                user,
                access_mode,
                file_descriptor: Some(fd),
                file_path,
            });
        }
    }

    Ok(processes)
}

#[cfg(target_os = "windows")]
fn get_file_users_windows(target_path: &Path) -> Result<Vec<ProcessInfo>> {
    use sysinfo::{PidExt, ProcessExt, System, SystemExt};

    let files = collect_files_recursively(target_path)?;
    let mut canonical_files = HashSet::new();

    for file in &files {
        if let Ok(canonical) = file.canonicalize() {
            canonical_files.insert(canonical);
        } else {
            canonical_files.insert(file.clone());
        }
    }

    let mut system = System::new_all();
    system.refresh_all();

    let mut processes = Vec::new();

    for (pid, process) in system.processes() {
        // Check if process executable path matches any target
        if let Some(exe_path) = process.exe() {
            let exe_canonical = exe_path
                .canonicalize()
                .unwrap_or_else(|_| exe_path.to_path_buf());

            for target_file in &canonical_files {
                if exe_canonical == *target_file
                    || target_file.starts_with(&exe_canonical)
                    || exe_canonical.starts_with(target_file)
                {
                    processes.push(ProcessInfo {
                        pid: pid.as_u32(),
                        name: process.name().to_string(),
                        user: process.user_id().map(|uid| uid.to_string()),
                        access_mode: Some("exe".to_string()),
                        file_descriptor: None,
                        file_path: Some(exe_path.to_path_buf()),
                    });
                    break;
                }
            }
        }

        // Check current working directory
        if let Some(cwd) = process.cwd() {
            let cwd_canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());

            for target_file in &canonical_files {
                if target_file.starts_with(&cwd_canonical) {
                    processes.push(ProcessInfo {
                        pid: pid.as_u32(),
                        name: process.name().to_string(),
                        user: process.user_id().map(|uid| uid.to_string()),
                        access_mode: Some("cwd".to_string()),
                        file_descriptor: None,
                        file_path: Some(cwd.to_path_buf()),
                    });
                    break;
                }
            }
        }
    }

    // Note: Windows file handle enumeration requires more complex API calls
    // and elevated permissions. For now, we use the basic sysinfo approach.
    // A future enhancement could use the Windows API directly.

    Ok(processes)
}

/// Tests of the Linux scan. They use a fake proc root, thus they compile and
/// run on Linux only. The ubuntu-latest CI job runs them.
#[cfg(all(test, target_os = "linux"))]
mod linux_scan_tests {
    use super::*;

    /// A numeric name, thus the procfs iterator treats the entry as a process.
    /// The entry is a regular file. procfs opens each process entry with
    /// `O_DIRECTORY`, thus the open fails.
    const UNOPENABLE_ENTRY_NAME: &str = "4242";

    /// The scan must skip an entry that procfs cannot open and go on.
    ///
    /// The fake proc root holds two entries. One is a regular file named
    /// `4242`, which procfs cannot open. The other is a symlink named for this
    /// test process. It points to the real entry of this process in `/proc`.
    /// This process holds `held.txt` open, thus the scan must find it past the
    /// bad entry.
    ///
    /// The order of directory entries is not fixed. That is fine. If the scan
    /// stops at the bad entry, it returns `Err` in both orders. If the scan
    /// skips the bad entry, it returns `Ok` with the holder in both orders.
    #[test]
    fn scan_skips_an_entry_it_cannot_open_and_finds_the_holder() {
        let dir = tempfile::tempdir().expect("create a temporary directory");

        let held_path = dir.path().join("held.txt");
        std::fs::write(&held_path, b"held").expect("write the held file");
        let _held = std::fs::File::open(&held_path).expect("open the held file");

        let fake_root = dir.path().join("proc");
        std::fs::create_dir(&fake_root).expect("create the fake proc root");
        std::fs::write(fake_root.join(UNOPENABLE_ENTRY_NAME), b"")
            .expect("write the unopenable entry");

        let pid = std::process::id().to_string();
        std::os::unix::fs::symlink(Path::new(PROC_ROOT).join(&pid), fake_root.join(&pid))
            .expect("link the entry of this process");

        let found = get_file_users_linux(&held_path, &fake_root).unwrap_or_else(|e| {
            panic!("the scan must skip an entry that procfs cannot open, but it failed with {e:#}")
        });

        let expected_path = held_path
            .canonicalize()
            .expect("canonicalize the held file");
        assert!(
            found.iter().any(|info| info.pid == std::process::id()
                && info.file_path.as_deref() == Some(expected_path.as_path())),
            "the scan must find this process as the holder of the file, but it found {found:?}"
        );
    }
}
