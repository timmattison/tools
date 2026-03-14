use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use ts_rs::TS;

use crate::error::NleError;

/// A terminal recording, compatible with the beta recorder format.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
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
#[ts(export)]
pub struct Event {
    pub time: f64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub data: String,
}

/// The type of a recording event.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum EventType {
    #[serde(rename = "o")]
    Output,
    #[serde(rename = "i")]
    Input,
}

/// Options for exporting a recording.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
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
#[ts(export)]
pub enum ExportFormat {
    Video,
    Web,
}

/// Response from loading a recording, includes both data and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoadedRecording {
    pub recording: Recording,
    pub metadata: RecordingMetadata,
}

/// Summary metadata about a loaded recording.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
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

        if file.metadata().map_err(|e| NleError::LoadError(e.to_string()))?.len() == 0 {
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
            serde_json::from_reader(decoder)
                .map_err(|e| NleError::InvalidFormat(format!("Failed to parse compressed recording: {e}")))
        } else {
            serde_json::from_reader(reader)
                .map_err(|e| NleError::InvalidFormat(format!("Failed to parse recording: {e}")))
        }
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
}
