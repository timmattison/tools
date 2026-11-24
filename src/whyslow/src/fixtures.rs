//! The facts of this Mac on 2026-11-15, from issue #687. The test modules of
//! the crate share them, so each module reads the same sample.

use std::path::PathBuf;
use std::time::Duration;

use faulte::vm::SwapUsage;

use crate::groups::Group;
use crate::sample::CpuTime;
use crate::tally::{GroupTotal, WorktreeTotal};
use crate::verdict::{CpuTicks, Facts, Pressure};

/// One gibibyte.
const GIB: u64 = 1 << 30;

/// One mebibyte.
const MIB: u64 = 1 << 20;

/// Makes the total of one worktree of the compilers.
fn worktree(path: &str, cpu: u64, memory_bytes: u64) -> WorktreeTotal {
    WorktreeTotal {
        worktree: Some(PathBuf::from(path)),
        processes: 1,
        cpu: CpuTime::from_centiseconds(cpu),
        memory_bytes,
        at_least: false,
    }
}

/// The sample of 2026-11-15 at 13:17: load 152 on 14 processors, 0%
/// idle, 44% system, warning pressure, 26 GiB in the compressor, and 3.9
/// of 5.0 GiB of swap in use. The groups are the ones that `top` hid.
pub(crate) fn sample_2026_11_15() -> Facts {
    let root = "/Volumes/SamsungSSDs/code/prediction-markets-worktrees";
    let worktrees = vec![
        worktree(&format!("{root}/issue-17"), 166, GIB + GIB / 2),
        worktree(&format!("{root}/issue-36"), 118, GIB * 4 / 5),
        worktree(&format!("{root}/issue-8"), 38, GIB * 2 / 5),
    ];
    let compiler_total = GroupTotal {
        group: Group::Compilers,
        processes: 68,
        accounts: 1,
        cpu: worktrees.iter().map(|w| w.cpu).sum(),
        memory_bytes: GIB * 27 / 10,
        at_least: false,
        worktrees,
    };
    let claude = GroupTotal {
        group: Group::ClaudeCode,
        processes: 176,
        accounts: 2,
        cpu: CpuTime::from_centiseconds(290),
        memory_bytes: 24 * GIB + GIB / 5,
        at_least: true,
        worktrees: Vec::new(),
    };
    let named = |name: &str, cpu: u64, memory_bytes: u64, at_least: bool| GroupTotal {
        group: Group::Named(name.to_owned()),
        processes: 1,
        accounts: 1,
        cpu: CpuTime::from_centiseconds(cpu),
        memory_bytes,
        at_least,
        worktrees: Vec::new(),
    };
    Facts {
        ticks: CpuTicks {
            user: 784,
            system: 616,
            idle: 0,
            nice: 0,
        },
        processors: 14,
        load: 152.0,
        window: Duration::from_secs(1),
        pressure: Pressure::Warning,
        swapouts: 1200,
        compressor_bytes: 26 * GIB,
        swap: SwapUsage {
            total_bytes: 5_368_709_120,
            used_bytes: 4_187_593_113,
        },
        groups: vec![
            claude,
            compiler_total,
            named("WindowServer", 75, 94 * MIB, true),
            named("wezterm-gui", 20, GIB * 34 / 10, false),
            named("Raycast Backend", 5, GIB * 9 / 10, false),
        ],
    }
}
