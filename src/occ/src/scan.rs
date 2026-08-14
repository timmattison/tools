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
pub fn encode_directory(_directory: &Path) -> String {
    String::new()
}

/// Reads the working directory a transcript records for itself.
///
/// The early bookkeeping lines of a transcript often carry a null directory, so
/// the first line with a non-empty one wins. Returns `None` when the transcript
/// records no directory at all.
#[must_use]
pub fn recorded_directory(_contents: &str) -> Option<String> {
    None
}

/// Reads the release that last wrote to a transcript.
///
/// The last record wins rather than the first, because a resumed session's early
/// records were written by whatever release ran it before.
#[must_use]
pub fn recorded_version(_contents: &str) -> Option<ClaudeVersion> {
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

impl Transcripts for ProjectTranscripts {
    fn for_directory(&self, _directory: &Path) -> Vec<Transcript> {
        Vec::new()
    }
}

/// Reads every process on the machine.
///
/// Returns all of them, not only the Claude Code ones: deciding what a process
/// is belongs to [`crate::classify`], and keeping that decision out of this
/// module is what lets it be tested without a live process table.
#[must_use]
pub fn gather_processes() -> Vec<ProcessFact> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{
        encode_directory, gather_processes, recorded_directory, recorded_version,
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
