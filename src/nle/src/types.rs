use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::error::NleError;
use crate::json_file;
use crate::pipeline::TerminalSize;

/// A terminal recording, compatible with the beta recorder format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub version: u32,
    pub width: u16,
    pub height: u16,
    pub timestamp: f64,
    pub duration: f64,
    pub command: String,
    pub title: String,
    pub env: BTreeMap<String, String>,
    pub events: Vec<Event>,
}

/// A single event in a recording (output or input).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub time: f64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub data: String,
}

/// The type of a recording event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "o")]
    Output,
    #[serde(rename = "i")]
    Input,
}

/// Options for exporting a recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    pub input_path: String,
    pub output_path: String,
    pub format: ExportFormat,
    pub theme: Option<String>,
    pub compress: Option<bool>,
    pub fps: Option<u32>,
    pub resolution: Option<String>,
    pub optimize_web: Option<bool>,
}

/// The format to export a recording to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    Video,
    Web,
}

/// Response from loading a recording, includes both data and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedRecording {
    pub recording: Recording,
    pub metadata: RecordingMetadata,
}

/// Summary metadata about a loaded recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub file_path: String,
    pub title: String,
    pub command: String,
    pub duration: f64,
    pub event_count: usize,
    pub width: u16,
    pub height: u16,
    pub timestamp: f64,
}

/// A time in seconds. It is finite and not below zero.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct Seconds(f64);

impl Seconds {
    /// The time zero.
    pub const ZERO: Seconds = Seconds(0.0);

    /// Make a time from a whole number of seconds. A `u32` always gives a
    /// finite time that is not below zero, so this constructor cannot fail,
    /// and a `const` can use it.
    #[must_use]
    pub const fn from_whole_seconds(value: u32) -> Self {
        Seconds(value as f64)
    }

    /// Make a time from a number of seconds.
    ///
    /// # Errors
    /// Returns `NleError::InvalidTime` when `value` is NaN, infinite, or below zero.
    pub fn new(value: f64) -> Result<Self, NleError> {
        if value.is_finite() && value >= 0.0 {
            Ok(Seconds(value))
        } else {
            Err(NleError::InvalidTime(value))
        }
    }

    /// The number of seconds.
    #[must_use]
    pub fn get(self) -> f64 {
        self.0
    }
}

impl TryFrom<f64> for Seconds {
    type Error = NleError;

    fn try_from(value: f64) -> Result<Self, NleError> {
        Seconds::new(value)
    }
}

impl From<Seconds> for f64 {
    fn from(value: Seconds) -> f64 {
        value.0
    }
}

/// The identifier of a clip. It is unique within one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClipId(pub u64);

/// The identifier of a timeline track. It is unique within one project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrackId(pub u64);

/// A clip: a range of one source recording, placed on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    pub id: ClipId,
    /// The source recording. It is shared and never changed, so the undo
    /// history and the clones of a project cost no copy of the events. A save
    /// writes each source once, and a load gives one shared source to all the
    /// clips that name it.
    pub source: Arc<Recording>,
    /// The start time within the source.
    pub in_point: Seconds,
    /// The end time within the source.
    pub out_point: Seconds,
    /// Where the clip starts on the timeline.
    pub timeline_offset: Seconds,
}

/// A track of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineTrack {
    pub id: TrackId,
    /// The clips on this track, by id, in the order they were put on it.
    pub clips: Vec<ClipId>,
}

/// The kind of a filter: its name in the filter registry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FilterKind(String);

impl FilterKind {
    /// Make a filter kind from its name.
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    /// The name of the filter kind.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The options of one filter: a JSON object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FilterOptions(serde_json::Map<String, serde_json::Value>);

impl FilterOptions {
    /// Make an empty set of options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the options with `key` set to `value`.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }

    /// The value of `key`, when it is set.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.0.get(key)
    }

    /// Parse the options as the type `O`.
    ///
    /// # Errors
    /// Returns the error of `serde_json` when the options do not parse as `O`.
    pub(crate) fn parse<O: serde::de::DeserializeOwned>(&self) -> Result<O, serde_json::Error> {
        serde_json::from_value(serde_json::Value::Object(self.0.clone()))
    }
}

/// The part of the timeline that a filter works on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FilterRegion {
    /// The whole timeline.
    All,
    /// The times from `start` up to, and not including, `end`.
    Range { start: Seconds, end: Seconds },
}

/// A filter that the project applies when it renders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedFilter {
    pub kind: FilterKind,
    pub region: FilterRegion,
    pub options: FilterOptions,
}

/// An edit project: clips of source recordings placed on tracks, and the
/// filters that apply to the result. It never changes a source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(
    into = "crate::project::ProjectFile",
    try_from = "crate::project::ProjectFile"
)]
pub struct EditProject {
    pub clips: Vec<Clip>,
    pub timeline: Vec<TimelineTrack>,
    pub filters: Vec<AppliedFilter>,
}

impl Recording {
    /// The size of the terminal that made this recording.
    #[must_use]
    pub fn size(&self) -> TerminalSize {
        TerminalSize {
            width: self.width,
            height: self.height,
        }
    }

    /// Load a recording from a file, auto-detecting gzip compression via magic bytes.
    ///
    /// # Errors
    /// Returns `NleError::LoadError` when the file cannot be opened or read,
    /// and `NleError::InvalidFormat` when the file is empty or does not parse
    /// as a recording.
    pub fn load(path: &Path) -> Result<Self, NleError> {
        json_file::load(path, "recording")
    }

    /// Save a recording to a file, optionally using gzip compression.
    ///
    /// The save is atomic: a save that fails leaves the old file as it was
    /// (see the private `json_file` module for the temp file and rename).
    ///
    /// # Errors
    /// Returns `NleError::SaveError` when the recording cannot be serialized,
    /// compressed, or written.
    pub fn save(&self, path: &Path, compress: bool) -> Result<(), NleError> {
        json_file::save(self, path, compress, "recording")
    }

    /// Extract metadata from this recording.
    pub fn metadata(&self, file_path: &str) -> RecordingMetadata {
        RecordingMetadata {
            file_path: file_path.to_string(),
            title: self.title.clone(),
            command: self.command.clone(),
            duration: self.duration,
            event_count: self.events.len(),
            width: self.width,
            height: self.height,
            timestamp: self.timestamp,
        }
    }

    /// Create a minimal recording for testing.
    #[cfg(test)]
    pub fn test_fixture() -> Self {
        Self {
            version: 1,
            width: 80,
            height: 24,
            timestamp: 1_700_000_000.0,
            duration: 5.0,
            command: "echo hello".to_string(),
            title: "Test Recording".to_string(),
            env: BTreeMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]),
            events: vec![
                Event {
                    time: 0.0,
                    event_type: EventType::Output,
                    data: "hello\r\n".to_string(),
                },
                Event {
                    time: 1.5,
                    event_type: EventType::Input,
                    data: "q".to_string(),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn test_load_plain_json() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        let json = serde_json::to_string_pretty(&recording).unwrap();
        std::fs::write(&path, &json).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.width, 80);
        assert_eq!(loaded.height, 24);
        assert_eq!(loaded.events.len(), 2);
        assert_eq!(loaded.title, "Test Recording");
        assert_eq!(loaded.command, "echo hello");
    }

    #[test]
    fn test_load_gzip() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json.gz");

        // Write gzip file
        let json = serde_json::to_string(&recording).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(json.as_bytes()).unwrap();
        encoder.finish().unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, 1);
        assert_eq!(loaded.events.len(), 2);
    }

    #[test]
    fn test_load_gzip_without_gz_extension() {
        // Magic byte detection should work regardless of extension
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.bin");

        let json = serde_json::to_string(&recording).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(json.as_bytes()).unwrap();
        encoder.finish().unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, 1);
    }

    #[test]
    fn test_load_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.json");
        std::fs::write(&path, "").unwrap();

        let result = Recording::load(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty"), "Expected 'empty' in error: {err}");
    }

    #[test]
    fn test_load_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not json at all").unwrap();

        let result = Recording::load(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Failed to parse"),
            "Expected parse error in: {err}"
        );
    }

    #[test]
    fn test_load_nonexistent_file() {
        let path = Path::new("/tmp/nonexistent-nle-test-file.json");
        let result = Recording::load(path);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_plain_json() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json");

        recording.save(&path, false).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, recording.version);
        assert_eq!(loaded.events.len(), recording.events.len());
    }

    #[test]
    fn test_save_gzip() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json.gz");

        recording.save(&path, true).unwrap();

        // Verify the file is actually gzip by checking magic bytes
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes[0], 0x1f, "Expected gzip magic byte 1");
        assert_eq!(bytes[1], 0x8b, "Expected gzip magic byte 2");

        // Verify it loads back correctly
        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, recording.version);
        assert_eq!(loaded.events.len(), recording.events.len());
    }

    #[test]
    fn test_save_load_roundtrip_preserves_data() {
        let original = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();

        // Test plain JSON roundtrip
        let json_path = dir.path().join("roundtrip.json");
        original.save(&json_path, false).unwrap();
        let loaded_json = Recording::load(&json_path).unwrap();
        assert_eq!(loaded_json.title, original.title);
        assert_eq!(loaded_json.command, original.command);
        assert_eq!(loaded_json.duration, original.duration);
        assert_eq!(loaded_json.env.get("SHELL"), Some(&"/bin/zsh".to_string()));

        // Test gzip roundtrip
        let gz_path = dir.path().join("roundtrip.json.gz");
        original.save(&gz_path, true).unwrap();
        let loaded_gz = Recording::load(&gz_path).unwrap();
        assert_eq!(loaded_gz.title, original.title);
        assert_eq!(loaded_gz.command, original.command);
    }

    #[test]
    fn test_metadata_extraction() {
        let recording = Recording::test_fixture();
        let meta = recording.metadata("/path/to/file.json");

        assert_eq!(meta.file_path, "/path/to/file.json");
        assert_eq!(meta.title, "Test Recording");
        assert_eq!(meta.command, "echo hello");
        assert_eq!(meta.duration, 5.0);
        assert_eq!(meta.event_count, 2);
        assert_eq!(meta.width, 80);
        assert_eq!(meta.height, 24);
    }

    #[test]
    fn test_event_type_serialization() {
        let event = Event {
            time: 1.0,
            event_type: EventType::Output,
            data: "test".to_string(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"o""#), "Expected type:o in {json}");

        let input_event = Event {
            time: 2.0,
            event_type: EventType::Input,
            data: "q".to_string(),
        };
        let json = serde_json::to_string(&input_event).unwrap();
        assert!(json.contains(r#""type":"i""#), "Expected type:i in {json}");
    }

    #[test]
    fn test_event_type_deserialization() {
        let json = r#"{"time":1.0,"type":"o","data":"hello"}"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event.event_type, EventType::Output));

        let json = r#"{"time":2.0,"type":"i","data":"q"}"#;
        let event: Event = serde_json::from_str(json).unwrap();
        assert!(matches!(event.event_type, EventType::Input));
    }

    #[test]
    fn test_save_compress_flag_overrides_extension() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();

        // Save with compress=true to a .json path (no .gz extension)
        let path = dir.path().join("compressed.json");
        recording.save(&path, true).unwrap();

        // File should be gzip despite the .json extension
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes[0], 0x1f, "Expected gzip magic byte 1");
        assert_eq!(bytes[1], 0x8b, "Expected gzip magic byte 2");

        // Should still load correctly via magic-byte detection
        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, recording.version);

        // Save with compress=false to a .gz path
        let path_gz = dir.path().join("uncompressed.json.gz");
        recording.save(&path_gz, false).unwrap();

        // File should be plain JSON despite the .gz extension
        let bytes = std::fs::read(&path_gz).unwrap();
        assert_ne!(bytes[0], 0x1f, "Should NOT be gzip");
    }

    #[test]
    fn test_two_saves_of_one_recording_give_the_same_bytes() {
        let mut recording = Recording::test_fixture();
        recording.env = (0..32)
            .map(|i| (format!("VAR_{i:02}"), i.to_string()))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (
            dir.path().join("first.json"),
            dir.path().join("second.json"),
        );
        recording.save(&first, false).unwrap();
        // A load builds a new map, so a map with a random order saves in a new order.
        Recording::load(&first)
            .unwrap()
            .save(&second, false)
            .unwrap();

        let text = std::fs::read_to_string(&first).unwrap();
        assert_eq!(text, std::fs::read_to_string(&second).unwrap());
        let positions: Vec<usize> = (0..32)
            .map(|i| text.find(&format!("\"VAR_{i:02}\"")).unwrap())
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "env is not in key order: {text}"
        );
    }

    #[test]
    fn test_utf8_safety_in_recording_data() {
        let mut recording = Recording::test_fixture();
        recording.title = "日本語テスト".to_string();
        recording.command = "echo café 🎉".to_string();
        recording.events[0].data = "出力データ\r\n".to_string();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf8.json");
        recording.save(&path, false).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.title, "日本語テスト");
        assert_eq!(loaded.command, "echo café 🎉");
        assert_eq!(loaded.events[0].data, "出力データ\r\n");
    }

    #[cfg(unix)]
    #[test]
    fn test_save_replaces_an_existing_file_instead_of_writing_into_it() {
        for compress in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let rec = dir.path().join("rec.json");
            let link = dir.path().join("link.json");
            Recording::test_fixture().save(&rec, compress).unwrap();
            std::fs::hard_link(&rec, &link).unwrap();

            let mut second = Recording::test_fixture();
            second.title = "Second".to_string();
            second.save(&rec, compress).unwrap();

            assert_eq!(Recording::load(&link).unwrap().title, "Test Recording");
            assert_eq!(Recording::load(&rec).unwrap().title, "Second");
            let mut names: Vec<String> = std::fs::read_dir(dir.path())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            assert_eq!(names, ["link.json", "rec.json"], "compress={compress}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_save_keeps_the_mode_of_an_existing_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rec.json");
        Recording::test_fixture().save(&path, false).unwrap();
        // 0o640 is neither the 0o600 of a temp file nor the 0o644 of a new
        // file under umask 022, so only a copy of the old mode gives it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();

        Recording::test_fixture().save(&path, false).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o640);
    }

    #[cfg(unix)]
    #[test]
    fn test_save_gives_a_new_file_the_mode_that_file_create_gives() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("reference.json");
        std::fs::File::create(&reference).unwrap();
        let path = dir.path().join("rec.json");

        Recording::test_fixture().save(&path, false).unwrap();

        let mode = |p: &Path| {
            let mode = std::fs::metadata(p).unwrap().permissions().mode();
            format!("{:o}", mode & 0o777)
        };
        assert_eq!(mode(&path), mode(&reference));
    }
}
