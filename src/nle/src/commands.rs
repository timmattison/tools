use std::path::PathBuf;

use tauri_plugin_shell::ShellExt;

use crate::error::NleError;
use crate::types::{ExportFormat, ExportOptions, LoadedRecording, Recording, RecordingMetadata};

/// Canonicalize a path and verify it exists, returning the resolved path.
fn validate_path(path: &str) -> Result<PathBuf, NleError> {
    let file_path = PathBuf::from(path);
    if !file_path.exists() {
        return Err(NleError::FileNotFound(path.to_string()));
    }
    file_path
        .canonicalize()
        .map_err(|e| NleError::LoadError(format!("Failed to resolve path {path}: {e}")))
}

/// Load a recording from a file path, returning both the recording and its metadata.
#[tauri::command]
pub async fn load_recording(path: String) -> Result<LoadedRecording, NleError> {
    load_recording_from(&path)
}

/// Load the recording at `path` and pair it with its metadata.
/// The metadata holds the canonical path of the file.
fn load_recording_from(path: &str) -> Result<LoadedRecording, NleError> {
    let file_path = validate_path(path)?;
    let recording = Recording::load(&file_path)?;
    let display_path = file_path.to_string_lossy();
    let metadata = recording.metadata(&display_path);
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
    save_recording_to(&path, &recording, compress)
}

/// Save `recording` to `path` after it resolves the parent directory.
/// An absent `compress` falls back to the `.gz` suffix of `path`.
fn save_recording_to(
    path: &str,
    recording: &Recording,
    compress: Option<bool>,
) -> Result<(), NleError> {
    let file_path = PathBuf::from(path);
    // Canonicalize the parent directory to prevent path traversal
    let parent = file_path.parent().ok_or_else(|| {
        NleError::SaveError(format!("Invalid path (no parent directory): {path}"))
    })?;
    if !parent.exists() {
        return Err(NleError::SaveError(format!(
            "Parent directory does not exist: {}",
            parent.display()
        )));
    }
    let canonical_parent = parent.canonicalize().map_err(|e| {
        NleError::SaveError(format!(
            "Failed to resolve parent directory {}: {e}",
            parent.display()
        ))
    })?;
    let file_name = file_path
        .file_name()
        .ok_or_else(|| NleError::SaveError(format!("Invalid path (no filename): {path}")))?;
    let canonical_path = canonical_parent.join(file_name);
    let should_compress = compress.unwrap_or_else(|| path.ends_with(".gz"));
    recording.save(&canonical_path, should_compress)
}

/// List recordings in a directory, returning metadata for each.
/// Only considers files ending in `.json` or `.json.gz`.
#[tauri::command]
pub async fn list_recordings(directory: String) -> Result<Vec<RecordingMetadata>, NleError> {
    list_recordings_in(&directory)
}

/// Read the metadata of each loadable `.json` or `.json.gz` file in `directory`.
/// The order of the result is not defined.
fn list_recordings_in(directory: &str) -> Result<Vec<RecordingMetadata>, NleError> {
    let dir_path = PathBuf::from(directory);
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
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
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
    validate_path(input_path).map_err(|e| match e {
        NleError::FileNotFound(_) => {
            NleError::FileNotFound(format!("Export input file not found: {input_path}"))
        }
        NleError::LoadError(msg) => NleError::ExportError(msg),
        other => other,
    })
}

/// Build the arguments that all `beta export` runs share: the subcommand,
/// the canonical input path, and the output path.
fn base_export_args(
    subcommand: &str,
    input_path: &str,
    output_path: &str,
) -> Result<Vec<String>, NleError> {
    let canonical_input = validate_export_input(input_path)?;
    Ok(vec![
        "export".to_string(),
        subcommand.to_string(),
        canonical_input.to_string_lossy().to_string(),
        "--output".to_string(),
        output_path.to_string(),
    ])
}

/// Build the full `beta` argument list for a video export.
/// It fails if the format is not Video or the input file is missing.
fn video_export_args(options: &ExportOptions) -> Result<Vec<String>, NleError> {
    if !matches!(options.format, ExportFormat::Video) {
        return Err(NleError::ExportError(
            "export_video requires Video format".into(),
        ));
    }

    let mut args = base_export_args("video", &options.input_path, &options.output_path)?;
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
    Ok(args)
}

/// Build the full `beta` argument list for a web export.
/// It fails if the format is not Web or the input file is missing.
fn web_export_args(options: &ExportOptions) -> Result<Vec<String>, NleError> {
    if !matches!(options.format, ExportFormat::Web) {
        return Err(NleError::ExportError(
            "export_web requires Web format".into(),
        ));
    }

    let mut args = base_export_args("web", &options.input_path, &options.output_path)?;
    if let Some(theme) = &options.theme {
        args.push("--theme".to_string());
        args.push(theme.clone());
    }
    if options.compress.unwrap_or(false) {
        args.push("--compress".to_string());
    }
    Ok(args)
}

/// Run the `beta` CLI with `args` through the shell plugin.
/// Returns `output_path` when `beta` succeeds.
async fn run_beta(
    app: &tauri::AppHandle,
    args: &[String],
    output_path: &str,
) -> Result<String, NleError> {
    let output = app
        .shell()
        .command("beta")
        .args(args)
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
    let args = video_export_args(&options)?;
    run_beta(&app, &args, &options.output_path).await
}

/// Export a recording to web (HTML) format by shelling out to the beta CLI.
#[tauri::command]
pub async fn export_web(app: tauri::AppHandle, options: ExportOptions) -> Result<String, NleError> {
    let args = web_export_args(&options)?;
    run_beta(&app, &args, &options.output_path).await
}

#[cfg(test)]
mod tests;
