use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::collections::HashSet;

#[derive(Parser)]
#[command(
    name = "wu",
    about = "Cross-platform tool to identify which processes have a file, directory, or device open",
    long_about = "wu (who's using) identifies which processes currently have a file, directory, or device open. Works on macOS, Linux, and Windows."
)]
struct Args {
    /// Path to check for open handles
    path: PathBuf,

    /// Output format as JSON
    #[arg(long, short)]
    json: bool,

    /// Verbose output with additional details
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub user: Option<String>,
    pub access_mode: Option<String>,
    pub file_descriptor: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    let processes = who_is_using(&args.path)
        .with_context(|| format!("Failed to check processes using path: {}", args.path.display()))?;
    
    if processes.is_empty() {
        if args.json {
            println!("[]");
        } else {
            println!("No processes found using: {}", args.path.display());
        }
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&processes)?);
    } else {
        print_human_readable(&processes, args.verbose);
    }

    Ok(())
}

fn print_human_readable(processes: &[ProcessInfo], verbose: bool) {
    println!("Processes using the specified path:");
    println!();
    
    if verbose {
        for process in processes {
            println!("PID: {}", process.pid);
            println!("Name: {}", process.name);
            if let Some(user) = &process.user {
                println!("User: {}", user);
            }
            if let Some(access) = &process.access_mode {
                println!("Access: {}", access);
            }
            if let Some(fd) = &process.file_descriptor {
                println!("File Descriptor: {}", fd);
            }
            println!();
        }
    } else {
        println!("{:<8} {:<20} {:<15} {}", "PID", "NAME", "USER", "ACCESS");
        println!("{}", "-".repeat(60));
        
        for process in processes {
            println!(
                "{:<8} {:<20} {:<15} {}",
                process.pid,
                truncate_string(&process.name, 20),
                process.user.as_deref().unwrap_or("unknown"),
                process.access_mode.as_deref().unwrap_or("unknown")
            );
        }
    }
}

fn truncate_string(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

#[cfg(target_os = "linux")]
fn who_is_using(path: &Path) -> Result<Vec<ProcessInfo>> {
    get_file_users_linux(path)
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

#[cfg(target_os = "linux")]
fn get_file_users_linux(target_path: &Path) -> Result<Vec<ProcessInfo>> {
    use procfs::process::all_processes;
    use std::fs;
    
    let target_canonical = target_path.canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());
    
    let mut processes = Vec::new();
    let mut seen_pids = HashSet::new();
    
    for process in all_processes()? {
        let process = process?;
        let pid = process.pid;
        
        // Skip if we've already processed this PID
        if seen_pids.contains(&pid) {
            continue;
        }
        
        // Get process info
        let stat = match process.stat() {
            Ok(stat) => stat,
            Err(_) => continue, // Process might have died
        };
        
        let status = match process.status() {
            Ok(status) => status,
            Err(_) => continue,
        };
        
        // Check file descriptors
        if let Ok(fds) = process.fd() {
            for fd_entry in fds {
                if let Ok(fd_info) = fd_entry {
                    if let Ok(fd_path) = fs::read_link(&fd_info.path()) {
                        let fd_canonical = fd_path.canonicalize()
                            .unwrap_or(fd_path);
                        
                        if fd_canonical == target_canonical || 
                           target_canonical.starts_with(&fd_canonical) ||
                           fd_canonical.starts_with(&target_canonical) {
                            
                            seen_pids.insert(pid);
                            processes.push(ProcessInfo {
                                pid: pid as u32,
                                name: stat.comm,
                                user: status.real_uid.map(|uid| format!("{}", uid)),
                                access_mode: Some(format!("fd:{}", fd_info.fd)),
                                file_descriptor: Some(format!("{}", fd_info.fd)),
                            });
                            break; // Found one match for this process, move to next
                        }
                    }
                }
            }
        }
        
        // Check memory maps
        if let Ok(maps) = process.maps() {
            for map in maps {
                if let Some(pathname) = map.pathname {
                    match pathname {
                        procfs::process::MMapPath::Path(map_path) => {
                            let map_canonical = map_path.canonicalize()
                                .unwrap_or(map_path);
                            
                            if map_canonical == target_canonical ||
                               target_canonical.starts_with(&map_canonical) ||
                               map_canonical.starts_with(&target_canonical) {
                                
                                if !seen_pids.contains(&pid) {
                                    seen_pids.insert(pid);
                                    processes.push(ProcessInfo {
                                        pid: pid as u32,
                                        name: stat.comm.clone(),
                                        user: status.real_uid.map(|uid| format!("{}", uid)),
                                        access_mode: Some("mmap".to_string()),
                                        file_descriptor: None,
                                    });
                                }
                                break;
                            }
                        }
                        _ => {} // Skip other pathname types
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
    
    let target_str = target_path.to_string_lossy();
    
    let output = Command::new("lsof")
        .args(&["-t", &target_str]) // -t for terse output (PIDs only)
        .output()
        .context("Failed to execute lsof command")?;
    
    if !output.status.success() {
        // lsof returns non-zero if no processes found, which is not an error for us
        return Ok(Vec::new());
    }
    
    let pids_output = String::from_utf8_lossy(&output.stdout);
    let mut processes = Vec::new();
    
    for line in pids_output.lines() {
        if let Ok(pid) = line.trim().parse::<u32>() {
            // Get detailed info for this PID
            let detail_output = Command::new("lsof")
                .args(&["-p", &pid.to_string(), &target_str])
                .output();
            
            if let Ok(detail) = detail_output {
                let detail_str = String::from_utf8_lossy(&detail.stdout);
                let info = parse_lsof_output(&detail_str, pid);
                if let Some(process_info) = info {
                    processes.push(process_info);
                }
            }
        }
    }
    
    Ok(processes)
}

#[cfg(target_os = "macos")]
fn parse_lsof_output(output: &str, pid: u32) -> Option<ProcessInfo> {
    for line in output.lines().skip(1) { // Skip header
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 9 {
            let name = fields[0].to_string();
            let user = Some(fields[2].to_string());
            let fd = fields[3].to_string();
            let access_mode = Some(fields[4].to_string());
            
            return Some(ProcessInfo {
                pid,
                name,
                user,
                access_mode,
                file_descriptor: Some(fd),
            });
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn get_file_users_windows(target_path: &Path) -> Result<Vec<ProcessInfo>> {
    use sysinfo::{System, SystemExt, ProcessExt, PidExt};
    
    let mut system = System::new_all();
    system.refresh_all();
    
    let target_canonical = target_path.canonicalize()
        .unwrap_or_else(|_| target_path.to_path_buf());
    
    let mut processes = Vec::new();
    
    for (pid, process) in system.processes() {
        // Check if process executable path matches
        if let Some(exe_path) = process.exe() {
            let exe_canonical = exe_path.canonicalize()
                .unwrap_or_else(|_| exe_path.to_path_buf());
            
            if exe_canonical == target_canonical ||
               target_canonical.starts_with(&exe_canonical) ||
               exe_canonical.starts_with(&target_canonical) {
                
                processes.push(ProcessInfo {
                    pid: pid.as_u32(),
                    name: process.name().to_string(),
                    user: process.user_id().map(|uid| uid.to_string()),
                    access_mode: Some("exe".to_string()),
                    file_descriptor: None,
                });
            }
        }
        
        // Check current working directory
        if let Some(cwd) = process.cwd() {
            let cwd_canonical = cwd.canonicalize()
                .unwrap_or_else(|_| cwd.to_path_buf());
            
            if target_canonical.starts_with(&cwd_canonical) {
                processes.push(ProcessInfo {
                    pid: pid.as_u32(),
                    name: process.name().to_string(),
                    user: process.user_id().map(|uid| uid.to_string()),
                    access_mode: Some("cwd".to_string()),
                    file_descriptor: None,
                });
            }
        }
    }
    
    // Note: Windows file handle enumeration requires more complex API calls
    // and elevated permissions. For now, we use the basic sysinfo approach.
    // A future enhancement could use the Windows API directly.
    
    Ok(processes)
}