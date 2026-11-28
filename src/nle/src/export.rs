//! The export of an edited project: the stages of an export, the settings of
//! a `beta export` run, and the read of the progress that `beta` reports.

use std::ffi::OsString;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use crate::commands::{
    run_beta, run_blocking, save_recording_to, video_export_args, web_export_args,
};
use crate::error::NleError;
use crate::types::{ExportFormat, ExportOptions, Recording};

/// A stage of an export, for a progress display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportProgress {
    /// The editor renders the project into one recording.
    Rendering,
    /// The rendered recording goes to its file. For a web page or a video,
    /// `beta` runs, and it reported no frame yet.
    Writing,
    /// `beta` wrote `done` of `total` frames.
    Frames {
        /// The number of frames that `beta` wrote.
        done: u64,
        /// The number of frames in the video.
        total: u64,
    },
}

/// A theme of `beta export`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// `beta` picks the colors.
    #[default]
    Auto,
    /// The Dracula colors.
    Dracula,
    /// The Monokai colors.
    Monokai,
    /// The dark Solarized colors.
    SolarizedDark,
    /// The light Solarized colors.
    SolarizedLight,
}

impl Theme {
    /// Each theme, in the order of a menu.
    pub const ALL: [Theme; 5] = [
        Theme::Auto,
        Theme::Dracula,
        Theme::Monokai,
        Theme::SolarizedDark,
        Theme::SolarizedLight,
    ];

    /// The name that `beta --theme` takes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Dracula => "dracula",
            Self::Monokai => "monokai",
            Self::SolarizedDark => "solarized-dark",
            Self::SolarizedLight => "solarized-light",
        }
    }
}

impl FromStr for Theme {
    type Err = NleError;

    /// Read a theme from the name that `beta --theme` takes.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` for any other text.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|theme| theme.as_str() == text)
            .ok_or_else(|| {
                NleError::ExportError(format!(
                    "the theme \"{text}\" is not one of auto, dracula, monokai, solarized-dark, solarized-light"
                ))
            })
    }
}

/// A video size in pixels. Both numbers are above zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolution {
    width: NonZeroU32,
    height: NonZeroU32,
}

impl FromStr for Resolution {
    type Err = NleError;

    /// Read `WIDTHxHEIGHT`, with a lowercase `x`, whole numbers above zero,
    /// and no spaces.
    ///
    /// # Errors
    /// Returns `NleError::ExportError` for any other text.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parse_error = || {
            NleError::ExportError(format!(
                "the resolution \"{text}\" is not WIDTHxHEIGHT, for example 1920x1080"
            ))
        };
        let (width, height) = text.split_once('x').ok_or_else(parse_error)?;
        Ok(Self {
            width: parse_dimension(width).ok_or_else(parse_error)?,
            height: parse_dimension(height).ok_or_else(parse_error)?,
        })
    }
}

/// Read one side of a resolution: digits only, and above zero.
fn parse_dimension(text: &str) -> Option<NonZeroU32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl fmt::Display for Resolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.width, self.height)
    }
}

/// What an export writes.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportTarget {
    /// A `beta` recording file. The name ends in `.json` or `.json.gz`.
    /// `compress: None` follows the `.gz` ending.
    Recording {
        /// Whether to gzip the file.
        compress: Option<bool>,
    },
    /// A web page with a player, through `beta export web`. The name ends in
    /// `.html`.
    Web {
        /// The colors of the page.
        theme: Theme,
        /// Whether `beta` compresses the recording in the page.
        compress: bool,
    },
    /// A video through `beta export video`. The name ends in `.mp4` or
    /// `.gif`, and `beta` picks the encoder from the ending.
    Video {
        /// The colors of the video.
        theme: Theme,
        /// The frame rate. `None` leaves the choice to `beta`.
        fps: Option<NonZeroU32>,
        /// The size of the video. `None` leaves the choice to `beta`.
        resolution: Option<Resolution>,
        /// Whether `beta` optimizes the video for the web.
        optimize_web: bool,
    },
}

/// One export: where it goes and what it writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRequest {
    /// The file to write.
    pub output_path: PathBuf,
    /// What to write.
    pub target: ExportTarget,
}

/// Write the rendered `recording` as `request` asks, and report each stage
/// through `progress`. The export reads `recording` and makes no copy of it.
///
/// The output path goes to `beta` and to the writer of a recording as text.
/// Thus an output path that is not valid UTF-8 is refused before the first
/// stage. `beta` gets the file at the end of a link as text too, thus a web
/// page or a video whose link ends at such a path is refused before the first
/// stage.
///
/// # Errors
/// Returns `NleError::ExportError` when the output path is not valid UTF-8,
/// `NleError::SaveError` when a recording file is refused or cannot be
/// written, and `NleError::ExportError` when the output path of a web page
/// or a video is refused, when the work file cannot be written, when `beta`
/// cannot start, or when `beta` fails. A panic in the work becomes
/// `NleError::TaskError`.
pub async fn export_recording(
    recording: Arc<Recording>,
    request: ExportRequest,
    progress: impl FnMut(ExportProgress) + Send + 'static,
) -> Result<PathBuf, NleError> {
    export_recording_with(recording, request, progress, std::env::var_os).await
}

/// The name of the work file that holds the rendered recording for `beta`.
const WORK_FILE_NAME: &str = "render.json";

/// `export_recording` with the variables of the environment from `var`, so
/// that a test finds a `beta` of its own.
pub(crate) async fn export_recording_with(
    recording: Arc<Recording>,
    request: ExportRequest,
    mut progress: impl FnMut(ExportProgress) + Send + 'static,
    var: impl Fn(&'static str) -> Option<OsString>,
) -> Result<PathBuf, NleError> {
    let output = output_path_text(&request.output_path)?.to_owned();
    let mut options = match &request.target {
        ExportTarget::Recording { compress } => {
            write_recording_file(recording, output, *compress, progress).await?;
            return Ok(request.output_path);
        }
        ExportTarget::Web { theme, compress } => ExportOptions {
            compress: Some(*compress),
            ..beta_options(output, ExportFormat::Web, *theme)
        },
        ExportTarget::Video {
            theme,
            fps,
            resolution,
            optimize_web,
        } => ExportOptions {
            fps: fps.map(NonZeroU32::get),
            resolution: resolution.map(|size| size.to_string()),
            optimize_web: Some(*optimize_web),
            ..beta_options(output, ExportFormat::Video, *theme)
        },
    };
    let work_dir = run_blocking(|| {
        tempfile::tempdir()
            .map_err(|e| NleError::ExportError(format!("Failed to make a work directory: {e}")))
    })
    .await?;
    let input = work_dir.path().join(WORK_FILE_NAME);
    let work_file = input.clone();
    run_blocking(move || {
        recording.save(&work_file, false).map_err(|e| {
            NleError::ExportError(format!("Failed to write the work file for beta: {e}"))
        })
    })
    .await?;
    options.input_path = input.to_string_lossy().into_owned();
    let args = match options.format {
        ExportFormat::Video => video_export_args(&options)?,
        ExportFormat::Web => web_export_args(&options)?,
    };

    progress(ExportProgress::Writing);
    run_beta(&args, &options.output_path, var, &mut progress).await?;
    // The work directory goes away here, and also when `?` returns early.
    drop(work_dir);
    Ok(request.output_path)
}

/// Write `recording` to `output_path` as a recording file, and report the
/// `Writing` stage. `compress: None` follows the `.gz` ending.
async fn write_recording_file(
    recording: Arc<Recording>,
    output_path: String,
    compress: Option<bool>,
    mut progress: impl FnMut(ExportProgress),
) -> Result<(), NleError> {
    progress(ExportProgress::Writing);
    run_blocking(move || save_recording_to(&output_path, &recording, compress)).await
}

/// The text of the output path `path`. `beta` and the writer of a recording
/// take a path as text, and a lossy conversion writes a file with a different
/// name.
///
/// # Errors
/// Returns `NleError::ExportError` when `path` is not valid UTF-8.
pub(crate) fn output_path_text(path: &Path) -> Result<&str, NleError> {
    path.to_str().ok_or_else(|| {
        NleError::ExportError(format!(
            "the output path {} is not valid UTF-8",
            path.display()
        ))
    })
}

/// The options of a `beta export` run of `format` to `output_path` with
/// `theme`. The input path is empty, and the caller sets the other options.
fn beta_options(output_path: String, format: ExportFormat, theme: Theme) -> ExportOptions {
    ExportOptions {
        input_path: String::new(),
        output_path,
        format,
        theme: Some(theme.as_str().to_string()),
        compress: None,
        fps: None,
        resolution: None,
        optimize_web: None,
    }
}

/// The text that starts a frame line of `beta`.
const FRAME_LINE_PREFIX: &str = "Processed frame ";
/// The text between the two numbers of a frame line of `beta`.
const FRAME_LINE_SEPARATOR: &str = " / ";

/// Read one line of the stdout of `beta`.
///
/// `Processed frame N / M` gives `Frames { done: N, total: M }` when both
/// numbers are whole, `M` is above zero, and `N` is not above `M`. Trailing
/// white space, such as a `\r`, is ignored. Any other line gives `None`.
pub(crate) fn parse_progress_line(line: &str) -> Option<ExportProgress> {
    let numbers = line.trim_end().strip_prefix(FRAME_LINE_PREFIX)?;
    let (done, total) = numbers.split_once(FRAME_LINE_SEPARATOR)?;
    let done: u64 = done.parse().ok()?;
    let total: u64 = total.parse().ok()?;
    (total > 0 && done <= total).then_some(ExportProgress::Frames { done, total })
}

#[cfg(test)]
mod tests;
