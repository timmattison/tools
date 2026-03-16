use std::path::PathBuf;

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
/// Uses gzip compression if `compress` is `Some(true)`, plain JSON if `Some(false)`,
/// or falls back to extension detection (`.gz` suffix) if `None`.
#[tauri::command]
pub async fn save_recording(
    path: String,
    recording: Recording,
    compress: Option<bool>,
) -> Result<(), NleError> {
    let file_path = PathBuf::from(&path);
    let should_compress = compress.unwrap_or_else(|| path.ends_with(".gz"));
    recording.save(&file_path, should_compress)
}

/// List recordings in a directory, returning metadata for each.
/// Only considers files ending in `.json` or `.json.gz`.
#[tauri::command]
pub async fn list_recordings(directory: String) -> Result<Vec<RecordingMetadata>, NleError> {
    let dir_path = PathBuf::from(&directory);
    if !dir_path.is_dir() {
        return Err(NleError::FileNotFound(format!(
            "Directory not found: {directory}"
        )));
    }

    let mut recordings = Vec::new();

    let entries = std::fs::read_dir(&dir_path)
        .map_err(|e| NleError::LoadError(format!("Failed to read directory: {e}")))?;

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "Warning: failed to read directory entry in {}: {e}",
                    dir_path.display()
                );
                continue;
            }
        };
        let path = entry.path();
        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("");
        if filename.ends_with(".json") || filename.ends_with(".json.gz") {
            if let Ok(recording) = Recording::load(&path) {
                recordings.push(recording.metadata(&path.to_string_lossy()));
            }
        }
    }

    Ok(recordings)
}

/// Validate that an export input path exists and return a canonicalized version.
fn validate_export_input(input_path: &str) -> Result<PathBuf, NleError> {
    let path = PathBuf::from(input_path);
    if !path.exists() {
        return Err(NleError::FileNotFound(format!(
            "Export input file not found: {input_path}"
        )));
    }
    path.canonicalize().map_err(|e| {
        NleError::ExportError(format!("Failed to resolve input path {input_path}: {e}"))
    })
}

/// Shell out to the `beta` CLI with the given subcommand and args.
async fn run_beta_export(
    app: &tauri::AppHandle,
    subcommand: &str,
    input_path: &str,
    output_path: &str,
    extra_args: Vec<String>,
) -> Result<String, NleError> {
    let canonical_input = validate_export_input(input_path)?;

    let mut args = vec![
        "export".to_string(),
        subcommand.to_string(),
        canonical_input.to_string_lossy().to_string(),
        "--output".to_string(),
        output_path.to_string(),
    ];
    args.extend(extra_args);

    let output = app
        .shell()
        .command("beta")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            NleError::ExportError(format!(
                "Failed to run beta CLI (is it installed and on PATH?): {e}"
            ))
        })?;

    if output.status.success() {
        Ok(output_path.to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(NleError::ExportError(format!(
            "beta export failed: {stderr}"
        )))
    }
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

    let mut extra_args = Vec::new();
    if let Some(theme) = &options.theme {
        extra_args.push("--theme".to_string());
        extra_args.push(theme.clone());
    }
    if let Some(fps) = options.fps {
        extra_args.push("--fps".to_string());
        extra_args.push(fps.to_string());
    }
    if let Some(resolution) = &options.resolution {
        extra_args.push("--resolution".to_string());
        extra_args.push(resolution.clone());
    }
    if options.optimize_web.unwrap_or(false) {
        extra_args.push("--optimize-web".to_string());
    }

    run_beta_export(&app, "video", &options.input_path, &options.output_path, extra_args).await
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

    let mut extra_args = Vec::new();
    if let Some(theme) = &options.theme {
        extra_args.push("--theme".to_string());
        extra_args.push(theme.clone());
    }
    if options.compress.unwrap_or(false) {
        extra_args.push("--compress".to_string());
    }

    run_beta_export(&app, "web", &options.input_path, &options.output_path, extra_args).await
}
