use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Parser)]
#[command(
    name = "wu",
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
            let paths_str = args.paths.iter()
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
                    println!("  - {} ({})", 
                        path.display(), 
                        proc.access_mode.as_deref().unwrap_or("unknown"));
                    if let Some(fd) = &proc.file_descriptor {
                        println!("    File Descriptor: {}", fd);
                    }
                }
            }
            println!();
        }
    } else {
        println!("{:<8} {:<20} {:<15} {:<10} {}", "PID", "NAME", "USER", "ACCESS", "FILE");
        println!("{}", "-".repeat(80));
        
        for process in processes {
            let file_str = process.file_path.as_ref()
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
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len - 3])
    }
}

fn collect_files_recursively(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    
    if path.is_file() {
        files.push(path.to_path_buf());
    } else if path.is_dir() {
        for entry in WalkDir::new(path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok()) {
            files.push(entry.path().to_path_buf());
        }
    } else {
        // It might be a device or special file
        files.push(path.to_path_buf());
    }
    
    Ok(files)
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
    use std::collections::HashSet;
    use std::fs;
    
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
    
    for process in all_processes()? {
        let process = process?;
        let pid = process.pid;
        
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
                            .unwrap_or(fd_path.clone());
                        
                        // Check if this fd matches any of our target files
                        for target_file in &canonical_files {
                            if fd_canonical == *target_file || 
                               target_file.starts_with(&fd_canonical) ||
                               fd_canonical.starts_with(target_file) {
                                
                                processes.push(ProcessInfo {
                                    pid: pid as u32,
                                    name: stat.comm.clone(),
                                    user: status.real_uid.map(|uid| format!("{}", uid)),
                                    access_mode: Some(format!("fd:{}", fd_info.fd)),
                                    file_descriptor: Some(format!("{}", fd_info.fd)),
                                    file_path: Some(fd_path.clone()),
                                });
                                break;
                            }
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
                                .unwrap_or(map_path.clone());
                            
                            // Check if this mmap matches any of our target files
                            for target_file in &canonical_files {
                                if map_canonical == *target_file ||
                                   target_file.starts_with(&map_canonical) ||
                                   map_canonical.starts_with(target_file) {
                                    
                                    processes.push(ProcessInfo {
                                        pid: pid as u32,
                                        name: stat.comm.clone(),
                                        user: status.real_uid.map(|uid| format!("{}", uid)),
                                        access_mode: Some("mmap".to_string()),
                                        file_descriptor: None,
                                        file_path: Some(map_path.clone()),
                                    });
                                    break;
                                }
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
    
    let mut all_processes = Vec::new();
    
    // Use +D for directories (recursive) and regular path for files
    let is_dir = target_path.is_dir();
    let target_str = target_path.to_string_lossy();
    
    let output = if is_dir {
        // Use +D for recursive directory search
        Command::new("lsof")
            .args(&["+D", &target_str])
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
    
    for line in output.lines().skip(1) { // Skip header
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 9 {
            let name = fields[0].to_string();
            let pid = fields[1].parse::<u32>()
                .context("Failed to parse PID")?;
            let user = Some(fields[2].to_string());
            let fd = fields[3].to_string();
            let access_mode = Some(fields[4].to_string());
            let file_path = Some(PathBuf::from(fields[8..]
                .join(" "))); // File path may contain spaces
            
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
    use sysinfo::{System, SystemExt, ProcessExt, PidExt};
    
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
            let exe_canonical = exe_path.canonicalize()
                .unwrap_or_else(|_| exe_path.to_path_buf());
            
            for target_file in &canonical_files {
                if exe_canonical == *target_file ||
                   target_file.starts_with(&exe_canonical) ||
                   exe_canonical.starts_with(target_file) {
                    
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
            let cwd_canonical = cwd.canonicalize()
                .unwrap_or_else(|_| cwd.to_path_buf());
            
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