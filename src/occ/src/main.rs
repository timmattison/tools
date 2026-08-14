//! Temporary probe: dump the process facts sysinfo exposes on this platform.

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

fn main() {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    let mut shown = 0;
    for (pid, process) in system.processes() {
        let name = process.name().to_string_lossy().to_string();
        let looks_versioned = name.split('.').count() == 3
            && name.split('.').all(|part| part.parse::<u64>().is_ok());
        if !looks_versioned || process.start_time() == 0 {
            continue;
        }
        println!("pid={pid}");
        println!("  name(p_comm) = {name}");
        println!("  exe          = {:?}", process.exe());
        println!("  cwd          = {:?}", process.cwd());
        println!("  start_time   = {}", process.start_time());
        println!("  run_time     = {}", process.run_time());
        let argv: Vec<String> = process
            .cmd()
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        println!("  argv[0..3]   = {:?}", &argv[..argv.len().min(3)]);
        shown += 1;
        if shown >= 4 {
            break;
        }
    }
    println!("total versioned-name processes seen: {}", shown);
}
