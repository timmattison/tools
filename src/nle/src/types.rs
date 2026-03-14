use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use ts_rs::TS;

use crate::error::NleError;

/// A terminal recording, compatible with the beta recorder format.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct Recording {
    pub version: u32,
    pub width: u16,
    pub height: u16,
    pub timestamp: f64,
    pub duration: f64,
    pub command: String,
    pub title: String,
    pub env: HashMap<String, String>,
    pub events: Vec<Event>,
}

/// A single event in a recording (output or input).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct Event {
    pub time: f64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub data: String,
}

/// The type of a recording event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub enum EventType {
    #[serde(rename = "o")]
    Output,
    #[serde(rename = "i")]
    Input,
}

/// Options for exporting a recording.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
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
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub enum ExportFormat {
    Video,
    Web,
}

/// Response from loading a recording, includes both data and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
pub struct LoadedRecording {
    pub recording: Recording,
    pub metadata: RecordingMetadata,
}

/// Summary metadata about a loaded recording.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/bindings/")]
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

impl Recording {
    /// Load a recording from a file, auto-detecting gzip compression via magic bytes.
    pub fn load(path: &Path) -> Result<Self, NleError> {
        let mut file = std::fs::File::open(path).map_err(|e| {
            NleError::LoadError(format!(
                "Failed to open recording file {}: {e}",
                path.display()
            ))
        })?;

        if file
            .metadata()
            .map_err(|e| NleError::LoadError(e.to_string()))?
            .len()
            == 0
        {
            return Err(NleError::InvalidFormat("Recording file is empty".into()));
        }

        // Check for gzip magic bytes (0x1f 0x8b) instead of relying on extension
        let mut magic = [0_u8; 2];
        let is_gzip = file.read_exact(&mut magic).is_ok() && magic == [0x1f, 0x8b];

        // Seek back to the beginning for the actual read
        file.seek(SeekFrom::Start(0))
            .map_err(|e| NleError::LoadError(e.to_string()))?;
        let reader = std::io::BufReader::new(file);

        if is_gzip {
            let decoder = flate2::read::GzDecoder::new(reader);
            serde_json::from_reader(decoder).map_err(|e| {
                NleError::InvalidFormat(format!("Failed to parse compressed recording: {e}"))
            })
        } else {
            serde_json::from_reader(reader).map_err(|e| {
                NleError::InvalidFormat(format!("Failed to parse recording: {e}"))
            })
        }
    }

    /// Save a recording to a file, using gzip compression if the path ends in `.gz`.
    pub fn save(&self, path: &Path) -> Result<(), NleError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| NleError::SaveError(format!("Failed to serialize recording: {e}")))?;

        let is_gzip = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|ext| ext == "gz");

        if is_gzip {
            let file = std::fs::File::create(path).map_err(|e| {
                NleError::SaveError(format!("Failed to create file {}: {e}", path.display()))
            })?;
            let mut encoder =
                flate2::write::GzEncoder::new(file, flate2::Compression::default());
            encoder.write_all(json.as_bytes()).map_err(|e| {
                NleError::SaveError(format!("Failed to write gzip file {}: {e}", path.display()))
            })?;
            encoder.finish().map_err(|e| {
                NleError::SaveError(format!(
                    "Failed to finish gzip file {}: {e}",
                    path.display()
                ))
            })?;
        } else {
            std::fs::write(path, json).map_err(|e| {
                NleError::SaveError(format!("Failed to write file {}: {e}", path.display()))
            })?;
        }

        Ok(())
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
            env: HashMap::from([("SHELL".to_string(), "/bin/zsh".to_string())]),
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
        let mut encoder =
            flate2::write::GzEncoder::new(file, flate2::Compression::default());
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
        let mut encoder =
            flate2::write::GzEncoder::new(file, flate2::Compression::default());
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

        recording.save(&path).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.version, recording.version);
        assert_eq!(loaded.events.len(), recording.events.len());
    }

    #[test]
    fn test_save_gzip() {
        let recording = Recording::test_fixture();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.json.gz");

        recording.save(&path).unwrap();

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
        original.save(&json_path).unwrap();
        let loaded_json = Recording::load(&json_path).unwrap();
        assert_eq!(loaded_json.title, original.title);
        assert_eq!(loaded_json.command, original.command);
        assert_eq!(loaded_json.duration, original.duration);
        assert_eq!(loaded_json.env.get("SHELL"), Some(&"/bin/zsh".to_string()));

        // Test gzip roundtrip
        let gz_path = dir.path().join("roundtrip.json.gz");
        original.save(&gz_path).unwrap();
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
    fn test_utf8_safety_in_recording_data() {
        let mut recording = Recording::test_fixture();
        recording.title = "日本語テスト".to_string();
        recording.command = "echo café 🎉".to_string();
        recording.events[0].data = "出力データ\r\n".to_string();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("utf8.json");
        recording.save(&path).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.title, "日本語テスト");
        assert_eq!(loaded.command, "echo café 🎉");
        assert_eq!(loaded.events[0].data, "出力データ\r\n");
    }
}
