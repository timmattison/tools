use std::ffi::OsString;
use std::path::PathBuf;

use crate::error::NleError;
use crate::types::{ExportFormat, ExportOptions, LoadedRecording, Recording, RecordingMetadata};

/// Run blocking file work on the blocking pool of the tokio runtime that the
/// caller runs on (dioxus-desktop starts a multi-thread tokio runtime), so that
/// the work does not hold a worker of that runtime.
/// A panic in the work becomes `NleError::TaskError`.
pub(crate) async fn run_blocking<T, F>(work: F) -> Result<T, NleError>
where
    F: FnOnce() -> Result<T, NleError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| NleError::TaskError(e.to_string()))?
}

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
///
/// # Errors
/// Returns `NleError::FileNotFound` when the path does not exist, and the
/// error of `Recording::load` when the file cannot be loaded. A panic in the
/// work becomes `NleError::TaskError`.
pub async fn load_recording(path: String) -> Result<LoadedRecording, NleError> {
    run_blocking(move || load_recording_from(&path)).await
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

/// File name endings that a saved recording may have.
const RECORDING_ENDINGS: &[&str] = &[".json", ".json.gz"];
/// File name endings that a video export may have.
const VIDEO_ENDINGS: &[&str] = &[".mp4", ".gif"];
/// File name endings that a web export may have.
const WEB_ENDINGS: &[&str] = &[".html"];
/// The file name ending of a gzip recording.
const GZIP_ENDINGS: &[&str] = &[".gz"];

/// Return true when `name`, in lowercase, ends with one of `endings`.
/// Each ending in `endings` must be lowercase.
fn has_ending(name: &str, endings: &[&str]) -> bool {
    let lowercase_name = name.to_lowercase();
    endings
        .iter()
        .any(|ending| lowercase_name.ends_with(ending))
}

/// Resolve an output path that nle writes, or that it asks `beta` to write.
///
/// It refuses a path whose file name, in lowercase, does not end with one of
/// `endings`. It refuses a path with no file name, or with no parent
/// directory, or whose parent directory does not exist. Otherwise it returns
/// the canonical parent directory joined with the file name. It does not
/// confine the path: a `..` or a symlink in the parent resolves to wherever it
/// points. Each error goes through `to_error`.
fn resolve_output_path(
    path: &str,
    endings: &[&str],
    to_error: fn(String) -> NleError,
) -> Result<PathBuf, NleError> {
    let file_path = PathBuf::from(path);
    let file_name = file_path
        .file_name()
        .ok_or_else(|| to_error(format!("Invalid path (no filename): {path}")))?;
    if !has_ending(&file_name.to_string_lossy(), endings) {
        return Err(to_error(format!(
            "Refusing to write {path}: the file name must end in one of {}",
            endings.join(", ")
        )));
    }
    let parent = file_path
        .parent()
        .ok_or_else(|| to_error(format!("Invalid path (no parent directory): {path}")))?;
    if !parent.exists() {
        return Err(to_error(format!(
            "Parent directory does not exist: {}",
            parent.display()
        )));
    }
    let canonical_parent = parent.canonicalize().map_err(|e| {
        to_error(format!(
            "Failed to resolve parent directory {}: {e}",
            parent.display()
        ))
    })?;
    Ok(canonical_parent.join(file_name))
}

/// Save `recording` to `path`. It refuses a file name that does not end in
/// `.json` or `.json.gz` in any case, and a parent directory that does not
/// exist. An absent `compress` falls back to the `.gz` suffix of the file
/// name, in any case.
pub(crate) fn save_recording_to(
    path: &str,
    recording: &Recording,
    compress: Option<bool>,
) -> Result<(), NleError> {
    let canonical_path = resolve_output_path(path, RECORDING_ENDINGS, NleError::SaveError)?;
    let should_compress =
        compress.unwrap_or_else(|| has_ending(&canonical_path.to_string_lossy(), GZIP_ENDINGS));
    recording.save(&canonical_path, should_compress)
}

/// List recordings in a directory, returning metadata for each.
/// Only considers files ending in `.json` or `.json.gz`, in any case.
///
/// # Errors
/// Returns `NleError::FileNotFound` when `directory` is not a directory, and
/// `NleError::LoadError` when it cannot be read. A panic in the work becomes
/// `NleError::TaskError`.
pub async fn list_recordings(directory: String) -> Result<Vec<RecordingMetadata>, NleError> {
    run_blocking(move || list_recordings_in(&directory)).await
}

/// Read the metadata of each loadable file in `directory` whose name ends in
/// `.json` or `.json.gz`, in any case.
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
        if has_ending(filename, RECORDING_ENDINGS) {
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
/// the canonical input path, and the resolved output path.
/// The output file name must end in one of `output_endings`.
fn base_export_args(
    subcommand: &str,
    input_path: &str,
    output_path: &str,
    output_endings: &[&str],
) -> Result<Vec<String>, NleError> {
    let canonical_input = validate_export_input(input_path)?;
    let resolved_output = resolve_output_path(output_path, output_endings, NleError::ExportError)?;
    Ok(vec![
        "export".to_string(),
        subcommand.to_string(),
        canonical_input.to_string_lossy().to_string(),
        "--output".to_string(),
        resolved_output.to_string_lossy().to_string(),
    ])
}

/// Build the full `beta` argument list for a video export.
/// It fails if the format is not Video, the input file is missing, or the
/// output file name does not end in `.mp4` or `.gif`, or the output directory
/// does not exist.
fn video_export_args(options: &ExportOptions) -> Result<Vec<String>, NleError> {
    if !matches!(options.format, ExportFormat::Video) {
        return Err(NleError::ExportError(
            "export_video requires Video format".into(),
        ));
    }

    let mut args = base_export_args(
        "video",
        &options.input_path,
        &options.output_path,
        VIDEO_ENDINGS,
    )?;
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
/// It fails if the format is not Web, the input file is missing, or the
/// output file name does not end in `.html`, or the output directory does not
/// exist.
fn web_export_args(options: &ExportOptions) -> Result<Vec<String>, NleError> {
    if !matches!(options.format, ExportFormat::Web) {
        return Err(NleError::ExportError(
            "export_web requires Web format".into(),
        ));
    }

    let mut args = base_export_args(
        "web",
        &options.input_path,
        &options.output_path,
        WEB_ENDINGS,
    )?;
    if let Some(theme) = &options.theme {
        args.push("--theme".to_string());
        args.push(theme.clone());
    }
    if options.compress.unwrap_or(false) {
        args.push("--compress".to_string());
    }
    Ok(args)
}

/// Directories where Homebrew puts binaries such as `ffmpeg`, which `beta`
/// runs. A macOS app that starts from Finder or the Dock does not have them
/// on its PATH.
const HOMEBREW_BIN_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];

/// The environment variable that lists the directories where a program is
/// found.
const PATH_VAR: &str = "PATH";

/// Build the PATH for the `beta` child from the variables that `var` gives
/// (`std::env::var_os` in the app): each entry of `PATH` in its order, then
/// `$CARGO_HOME/bin`, `$HOME/.cargo/bin`, and `HOMEBREW_BIN_DIRS`.
/// It adds no directory that is already an entry. It returns `None` when
/// `std::env::join_paths` fails, and the child then keeps the PATH of the app.
fn beta_path_env(var: impl Fn(&'static str) -> Option<OsString>) -> Option<OsString> {
    let mut entries: Vec<PathBuf> = var(PATH_VAR)
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let extras = var("CARGO_HOME")
        .map(|dir| PathBuf::from(dir).join("bin"))
        .into_iter()
        .chain(var("HOME").map(|dir| PathBuf::from(dir).join(".cargo").join("bin")))
        .chain(HOMEBREW_BIN_DIRS.iter().map(PathBuf::from));
    for extra in extras {
        if !entries.contains(&extra) {
            entries.push(extra);
        }
    }
    std::env::join_paths(entries).ok()
}

/// Build the `beta` command with `args`. Its PATH is the one that
/// `beta_path_env` builds from `var`. When `beta_path_env` gives `None`, the
/// command keeps the PATH of the app. The command sheds the inherited `GIT_`
/// variables, because `childenv::command` builds it. The child gets `/dev/null`
/// as its stdin, so it never reads the terminal of the app.
fn beta_command(
    args: &[String],
    var: impl Fn(&'static str) -> Option<OsString>,
) -> std::process::Command {
    let mut command = childenv::command("beta");
    command.args(args);
    command.stdin(std::process::Stdio::null());
    if let Some(path) = beta_path_env(var) {
        command.env(PATH_VAR, path);
    }
    command
}

/// Run the `beta` CLI with `args` as a child process.
/// Returns `output_path` when `beta` succeeds.
async fn run_beta(
    args: &[String],
    output_path: &str,
    var: impl Fn(&'static str) -> Option<OsString>,
) -> Result<String, NleError> {
    let output = tokio::process::Command::from(beta_command(args, var))
        .output()
        .await
        .map_err(|e| {
            NleError::ExportError(format!(
                "Failed to run beta CLI (is it on PATH or in ~/.cargo/bin?): {e}"
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
///
/// # Errors
/// Returns `NleError::FileNotFound` when the input is missing, and
/// `NleError::ExportError` when the format is not Video, the output path is
/// refused, `beta` cannot start, or `beta` fails.
pub async fn export_video(options: ExportOptions) -> Result<String, NleError> {
    let args = video_export_args(&options)?;
    run_beta(&args, &options.output_path, std::env::var_os).await
}

/// Export a recording to web (HTML) format by shelling out to the beta CLI.
///
/// # Errors
/// Returns `NleError::FileNotFound` when the input is missing, and
/// `NleError::ExportError` when the format is not Web, the output path is
/// refused, `beta` cannot start, or `beta` fails.
pub async fn export_web(options: ExportOptions) -> Result<String, NleError> {
    let args = web_export_args(&options)?;
    run_beta(&args, &options.output_path, std::env::var_os).await
}

#[cfg(test)]
mod tests;
