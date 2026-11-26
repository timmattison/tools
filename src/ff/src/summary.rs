//! The summary that `ff` writes to standard error at the end of a run.

use std::time::Duration;

use crate::plan::{SkipReason, SkippedMount};
use crate::walk::Report;

/// The volumes of a search of the whole machine.
#[derive(Debug, Clone, Copy)]
pub struct Volumes<'a> {
    /// The count of the volumes that the walk searched.
    pub searched: usize,
    /// The volumes that the plan did not walk.
    pub skipped: &'a [SkippedMount],
}

/// What a run tells at its end.
#[derive(Debug)]
pub struct Summary<'a> {
    /// What the walk did.
    pub report: &'a Report,
    /// The volumes of a search of the whole machine, or `None` for a search of
    /// named paths.
    pub volumes: Option<Volumes<'a>>,
    /// How long the walk took.
    pub elapsed: Duration,
    /// When true, the summary names each skipped directory and volume.
    pub show_skipped: bool,
}

impl Summary<'_> {
    /// The lines of the summary, each without its line end.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let report = self.report;
        let mut lines = Vec::new();
        if self.show_skipped {
            for skipped in self.volumes.map_or(&[][..], |volumes| volumes.skipped) {
                lines.push(format!(
                    "ff: volume skipped: {} ({})",
                    skipped.mount_point.display(),
                    describe(&skipped.reason)
                ));
            }
            for incomplete in &report.incomplete {
                lines.push(format!(
                    "ff: incomplete: {}: {}",
                    incomplete.path.display(),
                    incomplete.error
                ));
            }
            for denied in &report.denied {
                lines.push(format!("ff: denied: {}", denied.display()));
            }
            for cloud in &report.cloud_skipped {
                lines.push(format!("ff: cloud: {}", cloud.display()));
            }
        }

        lines.push(format!("ff: matches: {}", report.matches));
        lines.push(format!(
            "ff: entries read: {} in {:.1} s",
            report.entries,
            self.elapsed.as_secs_f64()
        ));
        if let Some(volumes) = self.volumes {
            lines.push(format!(
                "ff: volumes searched: {}, skipped: {}",
                volumes.searched,
                volumes.skipped.len()
            ));
        }
        lines.push(format!(
            "ff: incomplete subtrees: {}",
            report.incomplete.len()
        ));
        if !report.denied.is_empty() {
            lines.push(format!(
                "ff: denied directories: {} (System Integrity Protection and the privacy settings refuse some directories)",
                report.denied.len()
            ));
        }
        if !report.cloud_skipped.is_empty() {
            lines.push(format!(
                "ff: cloud directories skipped: {} (--include-cloud searches them)",
                report.cloud_skipped.len()
            ));
        }
        if report.retried_interruptions > 0 {
            lines.push(format!(
                "ff: interrupted calls retried: {}",
                report.retried_interruptions
            ));
        }

        let something_to_name = !report.incomplete.is_empty()
            || !report.denied.is_empty()
            || !report.cloud_skipped.is_empty()
            || self
                .volumes
                .is_some_and(|volumes| !volumes.skipped.is_empty());
        if something_to_name && !self.show_skipped {
            lines.push("ff: --show-skipped names each one".to_owned());
        }
        lines
    }
}

/// Tells in a few words why the plan skipped a volume.
fn describe(reason: &SkipReason) -> String {
    match reason {
        SkipReason::DeviceFiles => "device files".to_owned(),
        SkipReason::Automounter => "automounter map".to_owned(),
        SkipReason::SimulatorImage => "simulator image".to_owned(),
        SkipReason::SystemVolume => "system volume".to_owned(),
        SkipReason::InsideSkipped(outer) => format!("inside {}", outer.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Summary, Volumes};
    use crate::plan::{SkipReason, SkippedMount};
    use crate::walk::{Incomplete, Report};
    use std::io;
    use std::path::PathBuf;
    use std::time::Duration;

    fn report() -> Report {
        Report {
            entries: 10,
            matches: 2,
            ..Report::default()
        }
    }

    fn report_with_skips() -> Report {
        Report {
            incomplete: vec![Incomplete {
                path: PathBuf::from("/a"),
                error: io::Error::from_raw_os_error(libc::EIO),
            }],
            denied: vec![PathBuf::from("/b"), PathBuf::from("/c")],
            cloud_skipped: vec![PathBuf::from("/d")],
            retried_interruptions: 7,
            ..report()
        }
    }

    fn lines(report: &Report, volumes: Option<Volumes<'_>>, show_skipped: bool) -> Vec<String> {
        Summary {
            report,
            volumes,
            elapsed: Duration::from_millis(1500),
            show_skipped,
        }
        .lines()
    }

    #[test]
    fn a_complete_search_of_named_paths_tells_its_counts() {
        assert_eq!(
            lines(&report(), None, false),
            [
                "ff: matches: 2",
                "ff: entries read: 10 in 1.5 s",
                "ff: incomplete subtrees: 0",
            ]
        );
    }

    #[test]
    fn each_kind_of_skip_gets_a_count_and_a_hint_that_names_the_flag() {
        assert_eq!(
            lines(&report_with_skips(), None, false),
            [
                "ff: matches: 2",
                "ff: entries read: 10 in 1.5 s",
                "ff: incomplete subtrees: 1",
                "ff: denied directories: 2 (System Integrity Protection and the privacy settings refuse some directories)",
                "ff: cloud directories skipped: 1 (--include-cloud searches them)",
                "ff: interrupted calls retried: 7",
                "ff: --show-skipped names each one",
            ]
        );
    }

    #[test]
    fn show_skipped_names_each_skipped_path_before_the_counts() {
        let error = io::Error::from_raw_os_error(libc::EIO);
        assert_eq!(
            lines(&report_with_skips(), None, true),
            [
                format!("ff: incomplete: /a: {error}"),
                "ff: denied: /b".to_owned(),
                "ff: denied: /c".to_owned(),
                "ff: cloud: /d".to_owned(),
                "ff: matches: 2".to_owned(),
                "ff: entries read: 10 in 1.5 s".to_owned(),
                "ff: incomplete subtrees: 1".to_owned(),
                "ff: denied directories: 2 (System Integrity Protection and the privacy settings refuse some directories)".to_owned(),
                "ff: cloud directories skipped: 1 (--include-cloud searches them)".to_owned(),
                "ff: interrupted calls retried: 7".to_owned(),
            ]
        );
    }

    #[test]
    fn a_search_of_the_machine_tells_its_volumes() {
        let skipped = [
            SkippedMount {
                mount_point: PathBuf::from("/dev"),
                reason: SkipReason::DeviceFiles,
            },
            SkippedMount {
                mount_point: PathBuf::from("/home"),
                reason: SkipReason::Automounter,
            },
            SkippedMount {
                mount_point: PathBuf::from("/home/me"),
                reason: SkipReason::InsideSkipped(PathBuf::from("/home")),
            },
            SkippedMount {
                mount_point: PathBuf::from("/sim"),
                reason: SkipReason::SimulatorImage,
            },
            SkippedMount {
                mount_point: PathBuf::from("/System/Volumes/VM"),
                reason: SkipReason::SystemVolume,
            },
        ];
        let volumes = Volumes {
            searched: 3,
            skipped: &skipped,
        };

        assert_eq!(
            lines(&report(), Some(volumes), false),
            [
                "ff: matches: 2",
                "ff: entries read: 10 in 1.5 s",
                "ff: volumes searched: 3, skipped: 5",
                "ff: incomplete subtrees: 0",
                "ff: --show-skipped names each one",
            ]
        );
        assert_eq!(
            lines(&report(), Some(volumes), true)[..5],
            [
                "ff: volume skipped: /dev (device files)",
                "ff: volume skipped: /home (automounter map)",
                "ff: volume skipped: /home/me (inside /home)",
                "ff: volume skipped: /sim (simulator image)",
                "ff: volume skipped: /System/Volumes/VM (system volume)",
            ]
        );
    }
}
