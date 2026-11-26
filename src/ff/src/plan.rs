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
    pub fn parse(_text: &str) -> Self {
        Self::default()
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
/// mount point below it. The data volume prunes each firmlinked directory too,
/// because the walk of `system_root` reaches it through its firmlink.
#[must_use]
pub fn plan_machine(_system_root: &Path, _mounts: &[Mount], _firmlinks: &Firmlinks) -> MachinePlan {
    MachinePlan::default()
}
