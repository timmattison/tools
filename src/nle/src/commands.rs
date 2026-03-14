use std::path::{Path, PathBuf};

use tauri_plugin_shell::ShellExt;

use crate::error::NleError;
use crate::types::{ExportFormat, ExportOptions, LoadedRecording, Recording, RecordingMetadata};

/// Load a recording from a file path, returning both the recording and its metadata.
#[tauri::command]
pub async fn load_recording(path: String) -> Result<LoadedRecording, NleError> {
    let file_path = PathBuf::from(&path);
    if !file_path.exists() {
        return Err(NleError::FileNotFound(path));
    }
    let recording = Recording::load(&file_path)?;
    let metadata = recording.metadata(&path);
    Ok(LoadedRecording {
        recording,
        metadata,
    })
}

/// Save a recording to a file path.
/// Automatically uses gzip compression if the path ends in `.gz`.
#[tauri::command]
pub async fn save_recording(path: String, recording: Recording) -> Result<(), NleError> {
    let file_path = PathBuf::from(&path);
    recording.save(&file_path)
}

/// List recordings in a directory, returning metadata for each.
#[tauri::command]
pub async fn list_recordings(directory: String) -> Result<Vec<RecordingMetadata>, NleError> {
    let dir_path = Path::new(&directory);
    if !dir_path.is_dir() {
        return Err(NleError::FileNotFound(format!(
            "Directory not found: {directory}"
        )));
    }

    let mut recordings = Vec::new();

    let entries = std::fs::read_dir(dir_path)
        .map_err(|e| NleError::LoadError(format!("Failed to read directory: {e}")))?;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if matches!(ext, Some("json" | "gz")) {
            if let Ok(recording) = Recording::load(&path) {
                recordings.push(recording.metadata(&path.to_string_lossy()));
            }
        }
    }

    Ok(recordings)
}

/// Export a recording to video format by shelling out to the beta CLI.
#[tauri::command]
pub async fn export_video(
    app: tauri::AppHandle,
    options: ExportOptions,
) -> Result<String, NleError> {
    if !matches!(options.format, ExportFormat::Video) {
        return Err(NleError::ExportError(
            "export_video requires Video format".into(),
        ));
    }

    let mut args = vec![
        "export".to_string(),
        "video".to_string(),
        options.input_path.clone(),
        "--output".to_string(),
        options.output_path.clone(),
    ];

    if let Some(theme) = &options.theme {
        args.push("--theme".to_string());
        args.push(theme.clone());
    }
    if let Some(fps) = options.fps {
        args.push("--fps".to_string());
        args.push(fps.to_string());
    }
    if let Some(resolution) = &options.resolution {
        args.push("--resolution".to_string());
        args.push(resolution.clone());
    }
    if options.optimize_web.unwrap_or(false) {
        args.push("--optimize-web".to_string());
    }

    let output = app
        .shell()
        .command("beta")
        .args(&args)
        .output()
        .await
        .map_err(|e| NleError::ExportError(format!("Failed to run beta: {e}")))?;

    if output.status.success() {
        Ok(options.output_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(NleError::ExportError(format!(
            "beta export failed: {stderr}"
        )))
    }
}

/// Export a recording to web (HTML) format by shelling out to the beta CLI.
#[tauri::command]
pub async fn export_web(
    app: tauri::AppHandle,
    options: ExportOptions,
) -> Result<String, NleError> {
    if !matches!(options.format, ExportFormat::Web) {
        return Err(NleError::ExportError(
            "export_web requires Web format".into(),
        ));
    }

    let mut args = vec![
        "export".to_string(),
        "web".to_string(),
        options.input_path.clone(),
        "--output".to_string(),
        options.output_path.clone(),
    ];

    if let Some(theme) = &options.theme {
        args.push("--theme".to_string());
        args.push(theme.clone());
    }
    if options.compress.unwrap_or(false) {
        args.push("--compress".to_string());
    }

    let output = app
        .shell()
        .command("beta")
        .args(&args)
        .output()
        .await
        .map_err(|e| NleError::ExportError(format!("Failed to run beta: {e}")))?;

    if output.status.success() {
        Ok(options.output_path)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(NleError::ExportError(format!(
            "beta export failed: {stderr}"
        )))
    }
}
