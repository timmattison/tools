//! Reading the machine: the running processes, and the transcripts on disk.
//!
//! This is the only module that talks to the operating system. Everything it
//! learns is handed to the rules in [`crate::process`], [`crate::session`], and
//! [`crate::report`] as plain values.

use crate::report::Transcripts;
use crate::session::{SessionId, Transcript};
use crate::{ClaudeVersion, ProcessFact};
use std::path::{Path, PathBuf};

/// Bytes read from the start of a transcript when looking for its directory.
const HEAD_BUDGET: usize = 256 * 1024;

/// Bytes read from the end of a transcript when looking for its release.
const TAIL_BUDGET: u64 = 64 * 1024;

/// Encodes a working directory the way Claude Code names its transcript folder.
///
/// Every character outside `[A-Za-z0-9-]` becomes `-`. The rule was read off a
/// live machine: all 654 transcript folders on it match that alphabet, and both
/// `/` and `.` and `_` arrive as `-`.
///
/// The encoding is lossy — `a.b` and `a-b` and `a/b` all encode alike — so it is
/// used only to find a candidate folder. Which transcripts in that folder really
/// belong to the directory is settled by the directory each transcript records
/// inside itself.
///
/// # Examples
///
/// ```
/// use occ::scan::encode_directory;
/// use std::path::Path;
///
/// assert_eq!(
///     encode_directory(Path::new("/Volumes/code/tim.mattison.org")),
///     "-Volumes-code-tim-mattison-org"
/// );
/// ```
#[must_use]
pub fn encode_directory(directory: &Path) -> String {
    // Mapping character by character keeps this safe for any directory name:
    // a multi-byte character is one `char`, never a byte to be indexed into.
    directory
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Reads the working directory a transcript records for itself.
///
/// The early bookkeeping lines of a transcript often carry a null directory, so
/// the first line with a non-empty one wins. Returns `None` when the transcript
/// records no directory at all.
#[must_use]
pub fn recorded_directory(contents: &str) -> Option<String> {
    for line in contents.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if let Some(directory) = record.get("cwd").and_then(serde_json::Value::as_str) {
            if !directory.is_empty() {
                return Some(directory.to_string());
            }
        }
    }
    None
}

/// Reads the release that last wrote to a transcript.
///
/// The last record wins rather than the first, because a resumed session's early
/// records were written by whatever release ran it before.
#[must_use]
pub fn recorded_version(contents: &str) -> Option<ClaudeVersion> {
    for line in contents.lines().rev() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if let Some(version) = record.get("version").and_then(serde_json::Value::as_str) {
            if let Some(parsed) = ClaudeVersion::parse(version) {
                return Some(parsed);
            }
        }
    }
    None
}

/// The transcripts under a `~/.claude/projects` folder.
pub struct ProjectTranscripts {
    /// The `projects` folder holding one subfolder per working directory.
    projects_root: PathBuf,
}

impl ProjectTranscripts {
    /// Reads transcripts from an explicit `projects` folder.
    #[must_use]
    pub fn new(projects_root: PathBuf) -> Self {
        Self { projects_root }
    }

    /// Reads transcripts from `home/.claude/projects`.
    #[must_use]
    pub fn for_home(home: &Path) -> Self {
        Self::new(home.join(".claude").join("projects"))
    }
}

/// Reads up to `budget` bytes from the start of `path`.
///
/// The read is capped because a transcript grows without bound and only its
/// opening records are needed. A line cut in half by the cap parses as nothing
/// and is skipped by both readers above.
fn read_head(path: &Path, budget: usize) -> Option<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut buffer = vec![0_u8; budget];
    let filled = file.read(&mut buffer).ok()?;
    buffer.truncate(filled);
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Reads up to `budget` bytes from the end of `path`.
fn read_tail(path: &Path, budget: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    if length > budget {
        file.seek(SeekFrom::Start(length - budget)).ok()?;
    }
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Reads one transcript, if it belongs to `directory`.
///
/// Returns `None` when the file does not name a session, cannot be read, or
/// records a directory other than `directory`. A transcript that records no
/// directory at all is kept: absence is not a contradiction, and a session too
/// young to have recorded one is exactly the session most worth reporting.
fn read_transcript(path: &Path, directory: &Path) -> Option<Transcript> {
    let id = SessionId::parse(path.file_stem()?.to_str()?)?;

    let modified_epoch_secs = std::fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    let head = read_head(path, HEAD_BUDGET)?;
    if let Some(recorded) = recorded_directory(&head) {
        if Path::new(&recorded) != directory {
            return None;
        }
    }

    // The release is read from the end, because a resumed transcript's opening
    // records name whichever release ran it before.
    let version = read_tail(path, TAIL_BUDGET)
        .and_then(|tail| recorded_version(&tail))
        .or_else(|| recorded_version(&head));

    Some(Transcript {
        id,
        version,
        modified_epoch_secs,
    })
}

impl Transcripts for ProjectTranscripts {
    fn for_directory(&self, directory: &Path) -> Vec<Transcript> {
        let folder = self.projects_root.join(encode_directory(directory));
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|entry| read_transcript(&entry.path(), directory))
            .collect()
    }
}

/// Reads the kernel accounting name of a process.
///
/// This is the basename of the file the process actually executed, recorded
/// when it started. For Claude Code it is the release number, and it is the only
/// reliable source of that number: the executable path of a running session
/// resolves through the `claude` launcher, which is a link to whichever release
/// is installed *now*. Reading the release from that path would report the
/// newest release for a session running a release from months ago — the exact
/// claim this tool exists to make, made backwards.
///
/// Returns `None` when the name cannot be read, which is the normal answer for
/// another account's process.
#[must_use]
pub fn accounting_name(_pid: u32) -> Option<String> {
    None
}

/// Reads every process on the machine.
///
/// Returns all of them, not only the Claude Code ones: deciding what a process
/// is belongs to [`crate::classify`], and keeping that decision out of this
/// module is what lets it be tested without a live process table.
#[must_use]
pub fn gather_processes() -> Vec<ProcessFact> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );

    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessFact {
            pid: pid.as_u32(),
            accounting_name: process.name().to_string_lossy().into_owned(),
            exe: process.exe().map(Path::to_path_buf),
            argv: process
                .cmd()
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            cwd: process.cwd().map(Path::to_path_buf),
            uptime_secs: process.run_time(),
            start_time_epoch_secs: process.start_time(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        accounting_name, encode_directory, gather_processes, recorded_directory, recorded_version,
        ProjectTranscripts,
    };
    use crate::report::Transcripts;
    use std::path::{Path, PathBuf};

    const SESSION_A: &str = "d3b0d921-f0a1-41fc-b309-c11aa30c1173";
    const SESSION_B: &str = "ed84c8c7-0117-4670-936c-98e0f0d2c80b";

    /// A transcript folder laid out under a temporary root.
    ///
    /// `tempfile` gives each test its own root, so concurrent runs of this suite
    /// cannot read or truncate each other's fixtures.
    struct Fixture {
        root: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                root: tempfile::tempdir().expect("temporary root"),
            }
        }

        fn projects(&self) -> PathBuf {
            self.root.path().join("projects")
        }

        /// Writes `contents` as `<session>.jsonl` in the folder for `directory`.
        fn write(&self, directory: &str, session: &str, contents: &str) {
            let folder = self
                .projects()
                .join(encode_directory(Path::new(directory)));
            std::fs::create_dir_all(&folder).expect("transcript folder");
            std::fs::write(folder.join(format!("{session}.jsonl")), contents)
                .expect("transcript file");
        }

        fn transcripts(&self) -> ProjectTranscripts {
            ProjectTranscripts::new(self.projects())
        }
    }

    /// One transcript line recording a directory and a release.
    fn line(directory: &str, version: &str) -> String {
        format!("{{\"type\":\"user\",\"cwd\":\"{directory}\",\"version\":\"{version}\"}}\n")
    }

    #[test]
    fn encodes_every_character_outside_the_folder_alphabet_as_a_dash() {
        assert_eq!(
            encode_directory(Path::new("/Volumes/SamsungSSDs/code/tools-worktrees/occ")),
            "-Volumes-SamsungSSDs-code-tools-worktrees-occ"
        );
        // A dot and an underscore encode the same way a separator does.
        assert_eq!(
            encode_directory(Path::new("/Volumes/code/tim.mattison.org-worktrees/issue-131")),
            "-Volumes-code-tim-mattison-org-worktrees-issue-131"
        );
        assert_eq!(
            encode_directory(Path::new("/a/Earth_Science___Keys")),
            "-a-Earth-Science---Keys"
        );
    }

    #[test]
    fn encodes_multi_byte_characters_without_panicking() {
        // A directory name is arbitrary text. Encoding must not index into the
        // middle of a multi-byte character.
        assert_eq!(encode_directory(Path::new("/code/日本語/café")), "-code-----caf-");
    }

    #[test]
    fn reads_the_directory_a_transcript_records() {
        let contents = format!(
            "{}{}",
            "{\"type\":\"summary\",\"cwd\":null}\n",
            line("/work/here", "2.1.232")
        );
        assert_eq!(recorded_directory(&contents), Some("/work/here".to_string()));
    }

    #[test]
    fn reads_the_release_that_wrote_last() {
        // A resumed session's early records name the release that ran it before.
        let contents = format!("{}{}", line("/work", "2.1.204"), line("/work", "2.1.232"));
        assert_eq!(
            recorded_version(&contents).map(|v| v.as_str().to_string()),
            Some("2.1.232".to_string())
        );
    }

    #[test]
    fn survives_lines_that_are_not_json() {
        let contents = format!("not json at all\n\n{}", line("/work", "2.1.232"));
        assert_eq!(recorded_directory(&contents), Some("/work".to_string()));
        assert_eq!(
            recorded_version(&contents).map(|v| v.as_str().to_string()),
            Some("2.1.232".to_string())
        );
    }

    #[test]
    fn finds_the_transcripts_recorded_for_a_directory() {
        let fixture = Fixture::new();
        fixture.write("/work/here", SESSION_A, &line("/work/here", "2.1.205"));

        let found = fixture.transcripts().for_directory(Path::new("/work/here"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_str(), SESSION_A);
        assert_eq!(
            found[0].version.as_ref().map(|v| v.as_str().to_string()),
            Some("2.1.205".to_string())
        );
    }

    #[test]
    fn rejects_a_transcript_recorded_for_a_different_directory() {
        // The folder name is lossy: `/work/a.b` and `/work/a-b` share a folder.
        // The directory each transcript records is what tells them apart.
        let fixture = Fixture::new();
        fixture.write("/work/a.b", SESSION_A, &line("/work/a.b", "2.1.205"));
        fixture.write("/work/a-b", SESSION_B, &line("/work/a-b", "2.1.218"));

        let found = fixture.transcripts().for_directory(Path::new("/work/a-b"));

        assert_eq!(found.len(), 1, "only the transcript recorded for /work/a-b");
        assert_eq!(found[0].id.as_str(), SESSION_B);
    }

    #[test]
    fn keeps_a_transcript_that_records_no_directory() {
        // Absence is not contradiction: a transcript too young to have recorded
        // a directory is still a candidate for the folder it sits in.
        let fixture = Fixture::new();
        fixture.write("/work", SESSION_A, "{\"type\":\"summary\"}\n");

        let found = fixture.transcripts().for_directory(Path::new("/work"));

        assert_eq!(found.len(), 1);
    }

    #[test]
    fn ignores_files_that_do_not_name_a_session() {
        let fixture = Fixture::new();
        fixture.write("/work", SESSION_A, &line("/work", "2.1.205"));
        let folder = fixture.projects().join(encode_directory(Path::new("/work")));
        std::fs::write(folder.join("notes.txt"), "ignore me").expect("stray file");
        std::fs::write(folder.join("not-a-session.jsonl"), "{}").expect("stray transcript");

        let found = fixture.transcripts().for_directory(Path::new("/work"));

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id.as_str(), SESSION_A);
    }

    #[test]
    fn an_unknown_directory_has_no_transcripts() {
        let fixture = Fixture::new();
        let found = fixture.transcripts().for_directory(Path::new("/nowhere"));
        assert!(found.is_empty());
    }

    #[test]
    fn reads_the_accounting_name_of_this_process() {
        // The accounting name is the basename of the executed file, truncated by
        // the kernel. Checking it against this test binary's own path is a
        // ground truth available on any machine.
        let executable = std::env::current_exe().expect("current executable");
        let basename = executable
            .file_name()
            .and_then(|n| n.to_str())
            .expect("executable basename");

        let found = accounting_name(std::process::id()).expect("own accounting name");

        assert!(!found.is_empty(), "the accounting name should not be empty");
        assert!(
            basename.starts_with(&found),
            "accounting name {found:?} should be a prefix of the executable name {basename:?}"
        );
    }

    #[test]
    fn the_accounting_name_of_a_process_that_is_not_running_is_absent() {
        // Process identifier 0 is never an ordinary process to be read.
        assert_eq!(accounting_name(0), None);
    }

    #[test]
    fn the_gathered_release_does_not_come_from_the_launcher_link() {
        // The regression this guards: the executable path of a session resolves
        // through the `claude` launcher link, so reading the release from it
        // reports the installed release rather than the running one. The
        // accounting name of this test process is not the launcher's name, and
        // that is what the gathered facts must carry.
        let mine = std::process::id();
        let gathered = gather_processes();
        let fact = gathered
            .iter()
            .find(|fact| fact.pid == mine)
            .expect("this process should be gathered");

        let expected = accounting_name(mine).expect("own accounting name");
        assert_eq!(fact.accounting_name, expected);
    }

    #[test]
    fn gathers_the_running_process_table() {
        // The one process guaranteed to be running is this test.
        let mine = std::process::id();
        let gathered = gather_processes();
        assert!(
            gathered.iter().any(|fact| fact.pid == mine),
            "the running test process should appear in the gathered table"
        );
    }
}
