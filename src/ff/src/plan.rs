//! Which trees a search walks.
//!
//! A search of the whole machine gets its volumes from the mount table, and
//! walks each volume that holds user files one time. On macOS, the data volume
//! needs care. `/System/Volumes/Data` reports the same device id as `/`, thus a
//! device check does not stop a walk of `/` there. And each firmlink in
//! `/usr/share/firmlinks` shows a directory of the data volume at a second
//! path, for example `/Users` for `/System/Volumes/Data/Users`. The plan walks
//! each firmlinked directory through its firmlink, and walks the rest of the
//! data volume directly.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::walk::WalkRoot;

/// One entry of the mount table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The directory where the volume is mounted.
    pub mount_point: PathBuf,
    /// The type of the file system, for example `apfs`, `devfs`, or `autofs`.
    pub fs_type: String,
}

impl Mount {
    /// A mount of a file system of type `fs_type` at `mount_point`.
    #[must_use]
    pub fn new(mount_point: impl Into<PathBuf>, fs_type: &str) -> Self {
        Self {
            mount_point: mount_point.into(),
            fs_type: fs_type.to_owned(),
        }
    }
}

/// One line of `/usr/share/firmlinks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firmlink {
    /// The path that the system volume shows, for example `/Users`.
    pub system_path: PathBuf,
    /// The directory of the data volume that it shows, relative to the data
    /// volume, for example `Users`.
    pub data_path: PathBuf,
}

/// The firmlinks of the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Firmlinks(Vec<Firmlink>);

impl Firmlinks {
    /// Reads the text of `/usr/share/firmlinks`. Each line is the system path,
    /// a tab, and the data path.
    ///
    /// A line that does not have this form is skipped. A firmlink that the
    /// plan does not know costs a second walk of its directory, and never a
    /// lost directory.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        Self(
            text.lines()
                .filter_map(|line| {
                    let (system_path, data_path) = line.split_once('\t')?;
                    let (system_path, data_path) = (system_path.trim(), data_path.trim());
                    if system_path.is_empty() || data_path.is_empty() {
                        return None;
                    }
                    Some(Firmlink {
                        system_path: PathBuf::from(system_path),
                        data_path: PathBuf::from(data_path),
                    })
                })
                .collect(),
        )
    }

    /// Each firmlink, in the order of the file.
    #[must_use]
    pub fn entries(&self) -> &[Firmlink] {
        &self.0
    }
}

/// Why the plan does not walk a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// A `devfs` volume holds device files, not user files.
    DeviceFiles,
    /// An `autofs` map mounts a volume when a walk looks into it.
    Automounter,
    /// A read-only image of the iOS simulator, under
    /// `/Library/Developer/CoreSimulator`.
    SimulatorImage,
    /// A volume of the system, for example `/System/Volumes/VM`.
    SystemVolume,
    /// A volume inside a volume that the plan skips.
    InsideSkipped(PathBuf),
}

/// A volume that the plan does not walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedMount {
    /// The directory where the volume is mounted.
    pub mount_point: PathBuf,
    /// Why the plan does not walk it.
    pub reason: SkipReason,
}

/// The trees that a search of the whole machine walks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachinePlan {
    /// Each tree to walk, sorted by path.
    pub roots: Vec<WalkRoot>,
    /// Each volume that the plan does not walk, sorted by path.
    pub skipped: Vec<SkippedMount>,
}

/// Plans a search of the whole machine.
///
/// `system_root` is `/` on a real machine. Each special path of the rules is
/// below it: the data volume, the system volumes, and the simulator images. A
/// test gives a temporary directory, and the same rules apply there.
///
/// Each volume becomes one root that stays on its device, and that prunes each
/// mount point below it. The data volume hides each firmlinked directory: its
/// walk neither reports nor enters it, because the walk of `system_root`
/// reports it and enters it through its firmlink.
#[must_use]
pub fn plan_machine(system_root: &Path, mounts: &[Mount], firmlinks: &Firmlinks) -> MachinePlan {
    let mut unique: Vec<&Mount> = Vec::new();
    for mount in mounts {
        if !unique
            .iter()
            .any(|seen| seen.mount_point == mount.mount_point)
        {
            unique.push(mount);
        }
    }
    let direct: Vec<(&Mount, Option<SkipReason>)> = unique
        .iter()
        .map(|mount| (*mount, direct_skip_reason(system_root, mount)))
        .collect();

    let mut skipped = Vec::new();
    let mut walked = Vec::new();
    for (mount, reason) in &direct {
        let reason = reason.clone().or_else(|| {
            direct
                .iter()
                .find(|(outer, outer_reason)| {
                    outer_reason.is_some() && strictly_below(&mount.mount_point, &outer.mount_point)
                })
                .map(|(outer, _)| SkipReason::InsideSkipped(outer.mount_point.clone()))
        });
        match reason {
            Some(reason) => skipped.push(SkippedMount {
                mount_point: mount.mount_point.clone(),
                reason,
            }),
            None => walked.push(*mount),
        }
    }

    // A firmlinked directory is reached through its firmlink only when the
    // walk of the system root runs. Without that walk, the data volume must
    // cover it, thus it hides the directory only with that walk.
    let system_walked = walked.iter().any(|mount| mount.mount_point == system_root);
    let data_volume = system_root.join(DATA_VOLUME);
    let mut roots: Vec<WalkRoot> = walked
        .iter()
        .map(|mount| {
            let prune: BTreeSet<PathBuf> = unique
                .iter()
                .filter(|other| strictly_below(&other.mount_point, &mount.mount_point))
                .map(|other| other.mount_point.clone())
                .collect();
            let hide: BTreeSet<PathBuf> = if system_walked && mount.mount_point == data_volume {
                firmlinks
                    .entries()
                    .iter()
                    .map(|firmlink| {
                        let relative = firmlink
                            .data_path
                            .strip_prefix("/")
                            .unwrap_or(&firmlink.data_path);
                        data_volume.join(relative)
                    })
                    .collect()
            } else {
                BTreeSet::new()
            };
            WalkRoot {
                path: mount.mount_point.clone(),
                canonical: mount.mount_point.clone(),
                named: false,
                prune,
                hide,
                stay_on_device: true,
            }
        })
        .collect();
    roots.sort_by(|left, right| left.path.cmp(&right.path));
    skipped.sort_by(|left, right| left.mount_point.cmp(&right.mount_point));
    MachinePlan { roots, skipped }
}

/// The data volume of macOS, below the system root.
const DATA_VOLUME: &str = "System/Volumes/Data";

/// The directory of the read-only simulator images, below the system root.
const SIMULATOR_IMAGES: &str = "Library/Developer/CoreSimulator";

/// The volumes of the system, below the system root. They hold no user files.
const SYSTEM_VOLUMES: [&str; 6] = [
    "System/Volumes/VM",
    "System/Volumes/Preboot",
    "System/Volumes/Update",
    "System/Volumes/xarts",
    "System/Volumes/iSCPreboot",
    "System/Volumes/Hardware",
];

/// The type of a file system of device files.
const DEVICE_FILES_TYPE: &str = "devfs";

/// The type of an automounter map.
const AUTOMOUNTER_TYPE: &str = "autofs";

/// The rule that skips `mount` by itself, if one does.
fn direct_skip_reason(system_root: &Path, mount: &Mount) -> Option<SkipReason> {
    let path = &mount.mount_point;
    if mount.fs_type == DEVICE_FILES_TYPE {
        Some(SkipReason::DeviceFiles)
    } else if mount.fs_type == AUTOMOUNTER_TYPE {
        Some(SkipReason::Automounter)
    } else if path.starts_with(system_root.join(SIMULATOR_IMAGES)) {
        Some(SkipReason::SimulatorImage)
    } else if SYSTEM_VOLUMES
        .iter()
        .any(|volume| path.starts_with(system_root.join(volume)))
    {
        Some(SkipReason::SystemVolume)
    } else {
        None
    }
}

/// Tells if `path` is below `ancestor`, and is not `ancestor`.
fn strictly_below(path: &Path, ancestor: &Path) -> bool {
    path != ancestor && path.starts_with(ancestor)
}

/// Plans the walk of a directory that the user named.
///
/// `path` is the path as the user wrote it, and `canonical` is the same
/// directory with each symbolic link and each `..` resolved. Without
/// `one_file_system`, the walk crosses each mount point, as `find` does. With
/// it, the walk stays on the device of `path`, and it prunes each mount point
/// of `mounts` below `canonical`. A device check alone cannot stop at the data
/// volume of macOS, because it has the device id of `/`. The cloud rule reads
/// `canonical`, so a root such as `.` in `~/Library` skips the File Provider
/// directories.
#[must_use]
pub fn named_root(
    path: &Path,
    canonical: &Path,
    one_file_system: bool,
    mounts: &[Mount],
) -> WalkRoot {
    if !one_file_system {
        return WalkRoot {
            canonical: canonical.to_path_buf(),
            ..WalkRoot::new(path)
        };
    }
    let prune = mounts
        .iter()
        .filter_map(|mount| {
            let below = mount.mount_point.strip_prefix(canonical).ok()?;
            (!below.as_os_str().is_empty()).then(|| path.join(below))
        })
        .collect();
    WalkRoot {
        path: path.to_path_buf(),
        canonical: canonical.to_path_buf(),
        named: true,
        prune,
        hide: BTreeSet::new(),
        stay_on_device: true,
    }
}
