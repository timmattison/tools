use anyhow::{Context, Result};
use base64::prelude::*;
use buildinfo::version_string;
use clap::Parser;
use icy_sixel::{sixel_encode, EncodeOptions};
use image::DynamicImage;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};
use terminal_size::{terminal_size, Height, Width};
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::IntoRawMode;

// ============================================================================
// Constants for Sixel and terminal display calculations
// ============================================================================

/// Estimated pixel width per terminal character cell.
/// Used as fallback when actual pixel dimensions cannot be queried via ioctl.
/// Value of 10px is typical for modern terminals with default fonts.
const ESTIMATED_CELL_WIDTH_PX: u32 = 10;

/// Estimated pixel height per terminal character cell.
/// Used as fallback when actual pixel dimensions cannot be queried via ioctl.
/// Value of 20px is typical for modern terminals with default fonts (roughly 2:1 aspect).
const ESTIMATED_CELL_HEIGHT_PX: u32 = 20;

/// Default Sixel output width in pixels when no size information is available.
/// Used as final fallback when both ioctl and character cell estimates fail.
const DEFAULT_SIXEL_WIDTH_PX: u32 = 800;

/// Default Sixel output height in pixels when no size information is available.
/// Used as final fallback when both ioctl and character cell estimates fail.
const DEFAULT_SIXEL_HEIGHT_PX: u32 = 600;

/// Horizontal margin factor for Sixel output (95% of terminal width).
/// Leaves some margin to avoid horizontal overflow.
const SIXEL_HORIZONTAL_MARGIN: f64 = 0.95;

/// Vertical margin factor for Sixel output (90% of terminal height).
/// Leaves more vertical margin as some terminals have status bars or prompts.
const SIXEL_VERTICAL_MARGIN: f64 = 0.90;

/// Control sequence introducer. It starts a CSI escape sequence.
const CSI: &str = "\x1b[";

/// DECSC. It saves the position of the cursor.
const SAVE_CURSOR: &str = "\x1b7";

/// DECRC. It restores the saved position of the cursor.
const RESTORE_CURSOR: &str = "\x1b8";

/// The row that the shell prompt returns to below an image.
const PROMPT_ROWS: u32 = 1;

/// The number of rows that `ic` prints above an image.
///
/// The auto-fit path must subtract these rows from the height of the terminal.
/// If it does not, the header rows and the image together fill the screen, and
/// the terminal must scroll before the prompt can appear.
#[derive(Debug, Clone, Copy)]
struct HeaderRows(u32);

/// Give the number of rows that an auto-fit image can occupy.
///
/// The image gets the height of the terminal, less the header rows that `ic`
/// prints above it, less the row that the prompt returns to. The result has a
/// floor of 1, so a very short terminal still gets an image.
///
/// # Arguments
/// * `term_height` - The height of the terminal in rows.
/// * `header` - The number of rows that `ic` prints above the image.
///
/// # Returns
/// The number of rows for the image, which is always 1 or more.
fn auto_fit_rows(term_height: u32, header: HeaderRows) -> u32 {
    term_height
        .saturating_sub(header.0)
        .saturating_sub(PROMPT_ROWS)
        .max(1)
}

#[derive(Debug, Clone)]
enum VideoControl {
    Exit,
    TogglePause,
    FrameForward,
    FrameBackward,
    SeekForward(f64),  // seconds
    SeekBackward(f64), // seconds
}

/// Fast terminal image display utility with video playback
///
/// Performance Tips for Video Playback:
/// - Use --adaptive-fps to automatically adjust frame rate when terminal falls behind
/// - Use --max-fps to limit frame rate (e.g., --max-fps 15 for better performance)
/// - Use --scale to reduce image size (e.g., --scale 75 for 75% size)
/// - For high-resolution videos, try combining options: --scale 60 --max-fps 20
///
/// Terminal Compatibility:
/// - iTerm2: Uses iTerm2 inline image protocol
/// - Kitty: Uses Kitty graphics protocol (better performance for video)
/// - Ghostty: Uses Kitty graphics protocol (better performance for video)
/// - WezTerm: Uses Kitty graphics protocol
/// - Zellij: Uses Sixel protocol (works in Zellij running inside WezTerm or other Sixel-capable terminals)
/// - Alacritty: NOT SUPPORTED (text-only terminal, no graphics protocols)
/// - Other terminals: Limited or no image support
///
/// Optimizations for Real-time Video:
/// - Reduced protocol overhead with streamlined graphics commands
/// - Direct RGB data handling for Kitty terminals
/// - Efficient base64 encoding for iTerm2
/// - Minimized terminal control sequences
///
/// iTerm2 Settings for Better Performance:
/// - Disable "Background opacity" in Preferences > Profiles > Window
/// - Reduce scrollback buffer size in Preferences > Profiles > Terminal
#[derive(Parser, Debug)]
#[clap(version = version_string!(), about, long_about = None)]
struct Args {
    /// Image or video file(s) to display
    #[clap(value_name = "FILE")]
    files: Vec<PathBuf>,

    /// Width in characters (defaults to auto-sizing)
    #[clap(short, long)]
    width: Option<u32>,

    /// Height in characters (defaults to auto-sizing)
    #[clap(long)]
    height: Option<u32>,

    /// Preserve aspect ratio when resizing
    #[clap(long, default_value = "true")]
    preserve_aspect: bool,

    /// Read from stdin instead of file
    #[clap(long)]
    stdin: bool,

    /// Don't output newline after image
    #[clap(short, long)]
    no_newline: bool,

    /// Loop video playback
    #[clap(long)]
    loop_video: bool,

    /// Frame rate override for video playback (default: use video's frame rate)
    #[clap(long)]
    fps: Option<f64>,

    /// Disable frame dropping when video playback falls behind (keep all frames)
    #[clap(long)]
    do_not_drop_frames: bool,

    /// Frequency of memory cleanup during video playback (frames between cleanups, default: 60)
    #[clap(long, default_value = "60")]
    memory_cleanup_frequency: u32,

    /// Reduce image size for better terminal performance (percentage, 1-100, default: 100)
    #[clap(long, default_value = "100")]
    scale: u8,

    /// Maximum frame rate for video playback to improve terminal performance (default: use video's frame rate)
    #[clap(long)]
    max_fps: Option<f64>,

    /// Adaptive frame rate - automatically reduce FPS when terminal falls behind
    #[clap(long)]
    adaptive_fps: bool,

    /// Monitor directories for new images and display them automatically
    #[clap(long)]
    monitor: Vec<PathBuf>,

    /// Report whether this session can display an image, then exit. Exit code 0
    /// means yes. Exit code 1 means no, and the reason goes to stderr
    #[clap(long)]
    will_display: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    validate_arguments(&args)?;

    if args.will_display {
        report_display_readiness()?;
    } else if args.stdin {
        display_image_from_stdin(&args)?;
    } else if !args.files.is_empty() {
        for file_path in &args.files {
            ensure_file_exists(file_path)?;
            if is_video_file(file_path) {
                display_video_from_file(file_path, &args)?;
            } else if is_image_file(file_path) {
                // The callee prints the file name, so the auto-fit path can
                // count the row that the name takes.
                display_image_from_file(file_path, &args, &[file_path.display().to_string()])?;
            } else {
                // Treat as text file
                println!("{}", file_path.display());
                display_text_file(file_path)?;
            }
        }
    } else if !args.monitor.is_empty() {
        monitor_directories(&args.monitor, &args)?;
    }

    Ok(())
}

fn validate_arguments(args: &Args) -> Result<()> {
    validate_input_modes(args)?;
    validate_dimensions(args)?;
    validate_scale(args)?;
    validate_frame_options(args)?;
    validate_environment()?;
    Ok(())
}

fn validate_input_modes(args: &Args) -> Result<()> {
    let input_modes = [
        args.stdin,
        !args.files.is_empty(),
        !args.monitor.is_empty(),
        args.will_display,
    ];
    let input_count = input_modes.iter().filter(|&&x| x).count();

    if input_count == 0 {
        anyhow::bail!("Must specify a file, use --stdin, use --monitor, or use --will-display");
    }

    if input_count > 1 {
        anyhow::bail!(
            "Cannot specify multiple input modes (--stdin, file, --monitor, --will-display)"
        );
    }

    Ok(())
}

fn validate_dimensions(args: &Args) -> Result<()> {
    if let Some(width) = args.width {
        if width == 0 {
            anyhow::bail!("Width must be greater than 0");
        }
    }

    if let Some(height) = args.height {
        if height == 0 {
            anyhow::bail!("Height must be greater than 0");
        }
    }

    Ok(())
}

fn validate_scale(args: &Args) -> Result<()> {
    if args.scale == 0 || args.scale > 100 {
        anyhow::bail!("Scale must be between 1 and 100");
    }
    Ok(())
}

fn validate_frame_options(args: &Args) -> Result<()> {
    if args.do_not_drop_frames && args.adaptive_fps {
        anyhow::bail!("--do-not-drop-frames and --adaptive-fps are mutually exclusive");
    }
    Ok(())
}

fn validate_environment() -> Result<()> {
    // Environment validation that applies to all file types
    // tmux check moved to validate_terminal_for_graphics() since it only affects image/video display
    Ok(())
}

/// Verify that a path the user asked us to display actually points at a readable
/// file before we hand it off to a decoder or to `ffprobe`.
///
/// Without this check a missing path is routed by extension to whichever handler
/// matches — and for video files that means `ffprobe`, which we invoke with
/// `-v error`. A non-existent file then surfaces as a cryptic
/// "ffprobe failed to get video dimensions" instead of a plain "file not found".
/// Checking here gives every file type the same clear, actionable error.
fn ensure_file_exists(file_path: &Path) -> Result<()> {
    match fs::metadata(file_path) {
        Ok(metadata) => {
            if metadata.is_dir() {
                anyhow::bail!("{} is a directory, not a file", file_path.display());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("File not found: {}", file_path.display());
        }
        Err(error) => {
            Err(error).with_context(|| format!("Failed to access file: {}", file_path.display()))
        }
    }
}

fn monitor_directories(directories: &[PathBuf], args: &Args) -> Result<()> {
    // Validate that all directories exist
    for dir in directories {
        if !dir.exists() {
            anyhow::bail!("Directory does not exist: {}", dir.display());
        }
        if !dir.is_dir() {
            anyhow::bail!("Path is not a directory: {}", dir.display());
        }
    }

    println!("Monitoring directories for new images:");
    for dir in directories {
        println!("  - {}", dir.display());
    }
    println!("Press Ctrl+C to exit");

    // Create a channel to receive the events
    let (tx, rx) = mpsc::channel();

    // Create a watcher object, delivering debounced events
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())
        .context("Failed to create file watcher")?;

    // Add directories to watcher
    for dir in directories {
        watcher
            .watch(dir, RecursiveMode::Recursive)
            .with_context(|| format!("Failed to watch directory: {}", dir.display()))?;
    }

    // Keep track of recently displayed files to avoid duplicates
    let mut recent_files = HashSet::new();
    let mut last_cleanup = Instant::now();

    // Monitor for new files
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(event) => {
                if let Ok(event) = event {
                    match event.kind {
                        notify::EventKind::Create(_) | notify::EventKind::Modify(_) => {
                            for path in event.paths {
                                if !recent_files.contains(&path) {
                                    if is_image_file(&path) {
                                        // The callee prints the two header
                                        // rows, so the auto-fit path can count
                                        // them. The first row is empty, which
                                        // separates this image from the one
                                        // before it.
                                        let header = [
                                            String::new(),
                                            format!("Found new image: {}", path.display()),
                                        ];
                                        match display_image_from_file(&path, args, &header) {
                                            Ok(_) => {
                                                recent_files.insert(path.clone());
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "Failed to display image {}: {}",
                                                    path.display(),
                                                    e
                                                );
                                            }
                                        }
                                    } else if is_text_file(&path) {
                                        println!("\nFound new text file: {}", path.display());
                                        match display_text_file(&path) {
                                            Ok(_) => {
                                                recent_files.insert(path.clone());
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "Failed to display text file {}: {}",
                                                    path.display(),
                                                    e
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Clean up old entries from recent_files periodically
                if last_cleanup.elapsed() > Duration::from_secs(60) {
                    recent_files.clear();
                    last_cleanup = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    Ok(())
}

fn is_video_file(file_path: &Path) -> bool {
    if let Some(extension) = file_path.extension() {
        if let Some(ext_str) = extension.to_str() {
            let ext_lower = ext_str.to_lowercase();
            matches!(
                ext_lower.as_str(),
                "mp4"
                    | "avi"
                    | "mov"
                    | "mkv"
                    | "webm"
                    | "flv"
                    | "wmv"
                    | "m4v"
                    | "mpg"
                    | "mpeg"
                    | "3gp"
            )
        } else {
            false
        }
    } else {
        false
    }
}

fn is_image_file(file_path: &Path) -> bool {
    if let Some(extension) = file_path.extension() {
        if let Some(ext_str) = extension.to_str() {
            let ext_lower = ext_str.to_lowercase();
            matches!(
                ext_lower.as_str(),
                "jpg"
                    | "jpeg"
                    | "png"
                    | "gif"
                    | "bmp"
                    | "tiff"
                    | "tif"
                    | "webp"
                    | "svg"
                    | "ico"
                    | "ppm"
                    | "pbm"
                    | "pgm"
                    | "pnm"
            )
        } else {
            false
        }
    } else {
        false
    }
}

fn is_text_file(file_path: &Path) -> bool {
    // If it's not an image or video file, treat it as text
    !is_image_file(file_path) && !is_video_file(file_path)
}

fn ensure_ffmpeg_available() -> Result<()> {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        anyhow::bail!(
            "ffmpeg is required for video playback but was not found. Please install ffmpeg."
        );
    }
    Ok(())
}

fn ensure_ffprobe_available() -> Result<()> {
    if std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_err()
    {
        anyhow::bail!(
            "ffprobe is required for video playback but was not found. Please install ffmpeg."
        );
    }
    Ok(())
}

fn validate_terminal_for_graphics(
    terminal_caps: &TerminalCapabilities,
    transport: &RemoteTransport,
    feature: &str,
) -> Result<()> {
    // Check for Mosh first, since it strips escape sequences needed by all graphics protocols
    if *transport == RemoteTransport::Mosh {
        anyhow::bail!(
            "Mosh detected. {} display does not work over Mosh.\n\
            Mosh strips the escape sequences needed for image display (Sixel, Kitty, iTerm2).\n\
            \n\
            To display images, reconnect with ssh user@host instead of mosh user@host.",
            feature
        );
    }

    // Check for tmux, since graphics don't work in tmux
    if std::env::var("TMUX").is_ok() {
        anyhow::bail!("tmux detected. {} display does not work in tmux. Please run it directly in your terminal.", feature);
    }

    if !terminal_caps.supports_graphics {
        let term = std::env::var("TERM").unwrap_or_else(|_| "unknown".to_string());

        let error_msg = match terminal_caps.terminal_type {
            TerminalType::Alacritty => format!(
                "{} display is not supported in Alacritty terminal.\n\
                Alacritty is a text-only terminal that doesn't support graphics protocols.\n\
                \n\
                For {} display, please use one of these terminals:\n\
                • iTerm2 (macOS) - supports inline images\n\
                • Kitty - supports graphics protocol\n\
                • WezTerm - supports Kitty graphics protocol\n\
                \n\
                Alternatively, you can:\n\
                • Extract frames using ffmpeg and view them in an image viewer\n\
                • Use ASCII art video players like 'mplayer -vo caca' or 'vlc --intf dummy --vout caca'",
                feature, feature.to_lowercase()
            ),
            _ => format!(
                "{} display is not supported in this terminal.\n\
                This terminal doesn't support graphics protocols.\n\
                \n\
                For {} display, please use one of these terminals:\n\
                • iTerm2 (macOS) - supports inline images\n\
                • Kitty - supports graphics protocol\n\
                • WezTerm - supports Kitty graphics protocol\n\
                \n\
                Current terminal: {}\n\
                \n\
                Alternatively, you can:\n\
                • Extract frames using ffmpeg and view them in an image viewer\n\
                • Use ASCII art video players like 'mplayer -vo caca' or 'vlc --intf dummy --vout caca'",
                feature, feature.to_lowercase(), term
            )
        };

        anyhow::bail!("{}", error_msg);
    }
    Ok(())
}

/// Answer `--will-display`: can this session show an image?
///
/// The answer is the process exit status. `Ok` exits 0 and prints nothing.
/// An error exits 1 and prints the reason to stderr, so a script can write
/// `ic --will-display && ic picture.png` and get a usable message when the
/// answer is no.
///
/// The question goes to [`validate_terminal_for_graphics`], the same gate the
/// image path runs. Both callers therefore give one answer, and a session that
/// passes here cannot be refused by the next `ic picture.png`. The gate asks
/// about the terminal, the multiplexer, and the remote transport. It does not
/// ask whether stdout is a terminal, so a redirected stdout does not change
/// the answer.
fn report_display_readiness() -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    let transport = detect_remote_transport();

    validate_terminal_for_graphics(&terminal_caps, &transport, "Image")
}

fn display_video_from_file(file_path: &Path, args: &Args) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    let transport = detect_remote_transport();

    validate_terminal_for_graphics(&terminal_caps, &transport, "Video")?;
    ensure_ffmpeg_available()?;

    // Clear screen initially with function
    clear_screen()?;

    loop {
        // Get video info first to determine frame rate and duration
        let mut fps = if let Some(custom_fps) = args.fps {
            custom_fps
        } else {
            get_video_fps(file_path)?
        };

        // Apply max_fps limit if specified
        if let Some(max_fps) = args.max_fps {
            fps = fps.min(max_fps);
        }

        let duration = get_video_duration(file_path)?;
        let frame_duration = Duration::from_secs_f64(1.0 / fps);

        // Play video from the beginning with simple timing
        let playback_result = play_video_simple(file_path, frame_duration, args, duration, fps);

        // Handle any playback errors
        playback_result?;

        if !args.loop_video {
            break;
        }
    }

    Ok(())
}

/// Bundle of state created by [`setup_video_controls`]: optional raw-mode
/// guard, the channel receiving keyboard events, and the input thread handle.
type VideoControlSetup = (
    Option<termion::raw::RawTerminal<io::Stdout>>,
    std::sync::mpsc::Receiver<VideoControl>,
    Option<thread::JoinHandle<()>>,
);

fn setup_video_controls(terminal_caps: &TerminalCapabilities) -> Result<VideoControlSetup> {
    let supports_interactive_controls = terminal_caps.supports_raw_mode;

    if !supports_interactive_controls {
        print_control_notice(terminal_caps);
        let (_, rx) = std::sync::mpsc::channel();
        return Ok((None, rx, None));
    }

    // Set up raw mode for non-blocking input
    let raw_mode = io::stdout().into_raw_mode().ok();

    // Spawn a thread to handle keyboard input
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let input_handle = if raw_mode.is_some() {
        Some(thread::spawn(move || {
            let stdin = io::stdin();
            for key in stdin.keys().flatten() {
                let control = map_key_to_control(key);
                if let Some(ctrl) = control {
                    let is_exit = matches!(ctrl, VideoControl::Exit);
                    let _ = input_tx.send(ctrl);
                    if is_exit {
                        break;
                    }
                }
            }
        }))
    } else {
        None
    };

    Ok((raw_mode, input_rx, input_handle))
}

fn print_control_notice(terminal_caps: &TerminalCapabilities) {
    match terminal_caps.terminal_type {
        TerminalType::Alacritty => {
            eprintln!(
                "Notice: Running in Alacritty. Video will play without interactive controls."
            );
            eprintln!("For interactive video controls, consider using iTerm2 or Kitty.");
        }
        _ => {
            eprintln!("Notice: Terminal doesn't support interactive controls. Video will play automatically.");
        }
    }
    eprintln!("Press Ctrl+C to stop playback.");
}

fn map_key_to_control(key: Key) -> Option<VideoControl> {
    match key {
        Key::Esc | Key::Char('q') | Key::Char('Q') | Key::Ctrl('c') => Some(VideoControl::Exit),
        Key::Char(' ') => Some(VideoControl::TogglePause),
        Key::Left => Some(VideoControl::FrameBackward),
        Key::Right => Some(VideoControl::FrameForward),
        Key::Up => Some(VideoControl::SeekBackward(10.0)), // 10 seconds back
        Key::Down => Some(VideoControl::SeekForward(10.0)), // 10 seconds forward
        Key::Char('a') | Key::Char('A') => Some(VideoControl::SeekBackward(1.0)), // 1 second back
        Key::Char('d') | Key::Char('D') => Some(VideoControl::SeekForward(1.0)), // 1 second forward
        Key::Char('w') | Key::Char('W') => Some(VideoControl::SeekBackward(60.0)), // 1 minute back
        Key::Char('s') | Key::Char('S') => Some(VideoControl::SeekForward(60.0)), // 1 minute forward
        _ => None,
    }
}

fn handle_frame_timing(
    current_time: f64,
    expected_video_time: f64,
    fps: f64,
    frame_size: usize,
    reader: &mut BufReader<std::process::ChildStdout>,
    args: &Args,
    show_frame_after_seek: bool,
) -> Result<(f64, bool)> {
    // Skip timing logic when showing frame after seek
    if show_frame_after_seek {
        return Ok((current_time, false));
    }

    if current_time > expected_video_time {
        // We're ahead of schedule (video time > real time), wait
        let time_ahead = current_time - expected_video_time;
        thread::sleep(Duration::from_secs_f64(time_ahead));
        Ok((current_time, false))
    } else if current_time < expected_video_time && !args.do_not_drop_frames {
        // We're behind schedule - check if we should drop frames.
        // time_behind > 0 (from the branch condition) and fps >= 0, so the
        // product is non-negative; clamp to u32::MAX to avoid truncation panics
        // if it ever exceeds that bound.
        let time_behind = expected_video_time - current_time;
        let frames_behind_f = (time_behind * fps).clamp(0.0, f64::from(u32::MAX));
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "frames_behind_f is clamped to [0, u32::MAX] above"
        )]
        let frames_behind = frames_behind_f as u32;

        if frames_behind > 1 {
            // Skip frames to catch up
            let frames_to_skip = frames_behind.min(5); // Don't skip too many at once
            let new_time = current_time + frames_to_skip as f64 / fps;

            // Try to skip the frame data in ffmpeg output
            let mut skip_buffer = vec![0_u8; frame_size];
            for _ in 0..frames_to_skip {
                if reader.read_exact(&mut skip_buffer).is_err() {
                    break;
                }
            }
            Ok((new_time, true)) // true means we should continue to next iteration
        } else {
            Ok((current_time, false))
        }
    } else {
        Ok((current_time, false))
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "frame display needs image, args, timing, and three pieces of mutable per-call state; bundling into a struct just shifts boilerplate"
)]
fn process_frame_display(
    img: DynamicImage,
    args: &Args,
    current_time: f64,
    duration: f64,
    fps: f64,
    frames_since_clear: &mut u32,
    first_frame: &mut bool,
    previous_terminal_size: &mut Option<(u32, u32)>,
) -> Result<()> {
    // Periodic memory cleanup - clear scrollback to prevent memory buildup
    *frames_since_clear += 1;
    let cleanup_frequency = if fps > 30.0 {
        // More frequent cleanup for high FPS videos to manage memory
        args.memory_cleanup_frequency.min(30)
    } else {
        args.memory_cleanup_frequency
    };

    if *frames_since_clear >= cleanup_frequency {
        clear_scrollback()?;
        *frames_since_clear = 0;
    }

    // Check terminal size and decide on clearing strategy
    let current_terminal_size = get_terminal_size().ok();
    let should_clear_screen = if *first_frame {
        // Always clear for first frame
        *first_frame = false;
        true
    } else if let (Some(current), Some(previous)) = (current_terminal_size, *previous_terminal_size)
    {
        // Clear if terminal dimensions changed at all
        current.0 != previous.0 || current.1 != previous.1
    } else {
        // If we can't get terminal size, just use cursor positioning
        false
    };

    if should_clear_screen {
        clear_screen()?;
    } else {
        move_cursor_home()?;
    }

    // A video frame gets no header, so the image can use the whole terminal
    // less the row of the prompt.
    display_image(img, args, true, HeaderRows(0))?;

    // Draw progress bar
    if let Some((term_width, term_height)) = current_terminal_size {
        draw_progress_bar(current_time, duration, fps, term_width, term_height)?;
    }

    // Update previous terminal size for next comparison
    *previous_terminal_size = current_terminal_size;

    Ok(())
}

fn update_adaptive_fps(
    args: &Args,
    fps: &mut f64,
    original_fps: f64,
    consecutive_late_frames: &mut u32,
    adaptive_fps_active: &mut bool,
    last_display_time: &mut Instant,
) {
    if !args.adaptive_fps {
        return;
    }

    let display_time = last_display_time.elapsed();
    let expected_frame_time = Duration::from_secs_f64(1.0 / *fps);

    if display_time > expected_frame_time * 2 {
        // Frame took more than 2x the expected time - we're falling behind
        *consecutive_late_frames += 1;

        if *consecutive_late_frames >= 5 && !*adaptive_fps_active {
            // Reduce FPS to help terminal keep up
            *fps = (*fps * 0.75).max(10.0); // Don't go below 10 FPS
            *adaptive_fps_active = true;
        }
    } else {
        *consecutive_late_frames = 0;

        // If we've been adaptive and frames are smooth, gradually increase FPS
        if *adaptive_fps_active && *consecutive_late_frames == 0 {
            *fps = (*fps * 1.05).min(original_fps);
            if *fps >= original_fps * 0.95 {
                *fps = original_fps;
                *adaptive_fps_active = false;
            }
        }
    }

    *last_display_time = Instant::now();
}

/// RAII guard that restores the main screen buffer and shows the cursor on drop.
/// This ensures terminal state is restored even if video playback panics.
struct AlternateScreenGuard;

impl AlternateScreenGuard {
    fn enter() -> Result<Self> {
        // Switch to alternate screen buffer and hide cursor for clean full-screen rendering.
        // The alternate screen buffer prevents scrollback accumulation and eliminates
        // ghost progress bars caused by terminal scrolling during image display.
        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()?;
        Ok(Self)
    }
}

impl Drop for AlternateScreenGuard {
    fn drop(&mut self) {
        // Restore main screen buffer and show cursor.
        // Ignore flush errors — best effort during cleanup.
        print!("\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
    }
}

fn play_video_simple(
    file_path: &Path,
    _frame_duration: Duration,
    args: &Args,
    duration: f64,
    fps: f64,
) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    let (_raw_mode, input_rx, _input_handle) = setup_video_controls(&terminal_caps)?;

    let _screen_guard = AlternateScreenGuard::enter()?;

    play_video_inner(file_path, args, duration, fps, &input_rx)
}

fn play_video_inner(
    file_path: &Path,
    args: &Args,
    duration: f64,
    mut fps: f64,
    input_rx: &std::sync::mpsc::Receiver<VideoControl>,
) -> Result<()> {
    let mut current_time = 0.0; // Current position in the video (in seconds)
    let mut is_paused = false;
    let mut previous_terminal_size: Option<(u32, u32)> = None;
    let mut first_frame = true;
    let mut show_frame_after_seek = false; // Flag to show one frame after seeking
    let mut frames_since_clear = 0; // Track frames for periodic memory cleanup

    // Adaptive FPS variables
    let original_fps = fps;
    let mut consecutive_late_frames = 0;
    let mut last_display_time = Instant::now();
    let mut adaptive_fps_active = false;

    // Track timing more precisely - will be reset on seek operations
    let mut playback_start_time: Instant;
    let mut playback_start_video_time: f64; // Video time when current playback segment started
    let mut pause_start_time: Option<Instant> = None;
    let mut total_paused_duration: Duration;

    // Main playback loop - restart FFmpeg when resuming from pause or seeking
    'main_loop: loop {
        // Reset timing when starting new playback segment (after seek or initial start)
        playback_start_time = Instant::now();
        playback_start_video_time = current_time;
        total_paused_duration = Duration::from_secs(0);

        // Start ffmpeg from current position
        let (video_width, video_height) = get_video_dimensions(file_path)?;

        let mut ffmpeg_child = std::process::Command::new("ffmpeg")
            .args([
                "-i",
                file_path.to_str().unwrap(),
                "-ss",
                &format!("{:.6}", current_time), // Seek to current position with higher precision
                "-avoid_negative_ts",
                "make_zero",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to start ffmpeg")?;

        let stdout = ffmpeg_child
            .stdout
            .take()
            .context("Failed to get ffmpeg stdout")?;

        let mut reader = BufReader::new(stdout);
        let frame_size = (video_width * video_height * 3) as usize;
        let mut frame_buffer = vec![0_u8; frame_size];

        // Read and display frames until paused, finished, or exit
        'frame_loop: loop {
            // Check for user input (non-blocking)
            match input_rx.try_recv() {
                Ok(VideoControl::Exit) => {
                    let _ = ffmpeg_child.kill();
                    break 'main_loop;
                }
                Ok(VideoControl::TogglePause) => {
                    if is_paused {
                        // Resume - restart ffmpeg, update timing
                        is_paused = false;
                        if let Some(start) = pause_start_time {
                            total_paused_duration += start.elapsed();
                            pause_start_time = None;
                        }
                        let _ = ffmpeg_child.kill();
                        break 'frame_loop; // Restart ffmpeg
                    } else {
                        // Pause - track when we paused
                        is_paused = true;
                        pause_start_time = Some(Instant::now());
                        let _ = ffmpeg_child.kill();

                        // Wait for unpause or other commands
                        while is_paused {
                            match input_rx.recv_timeout(Duration::from_millis(100)) {
                                Ok(VideoControl::Exit) => break 'main_loop,
                                Ok(VideoControl::TogglePause) => {
                                    if let Some(start) = pause_start_time {
                                        total_paused_duration += start.elapsed();
                                        pause_start_time = None;
                                    }
                                    is_paused = false;
                                    break;
                                }
                                Ok(VideoControl::FrameForward) => {
                                    // Move forward one frame (1/fps seconds) and stay paused
                                    current_time += 1.0 / fps;
                                    if current_time >= duration {
                                        current_time = duration - (1.0 / fps); // Stay on last frame
                                    }
                                    show_frame_after_seek = true;
                                    // Keep video paused after seeking
                                    pause_start_time = Some(Instant::now()); // Reset pause timer for consistent timing
                                    let _ = ffmpeg_child.kill();
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::FrameBackward) => {
                                    // Move backward one frame (1/fps seconds) and stay paused
                                    current_time -= 1.0 / fps;
                                    current_time = current_time.max(0.0);
                                    show_frame_after_seek = true;
                                    // Keep video paused after seeking
                                    pause_start_time = Some(Instant::now()); // Reset pause timer for consistent timing
                                    let _ = ffmpeg_child.kill();
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::SeekForward(seconds)) => {
                                    current_time += seconds;
                                    if current_time >= duration {
                                        current_time = duration - (1.0 / fps); // Stay on last frame
                                    }
                                    show_frame_after_seek = true;
                                    // Keep video paused after seeking
                                    pause_start_time = Some(Instant::now()); // Reset pause timer for consistent timing
                                    let _ = ffmpeg_child.kill();
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::SeekBackward(seconds)) => {
                                    current_time -= seconds;
                                    current_time = current_time.max(0.0);
                                    show_frame_after_seek = true;
                                    // Keep video paused after seeking
                                    pause_start_time = Some(Instant::now()); // Reset pause timer for consistent timing
                                    let _ = ffmpeg_child.kill();
                                    break; // Restart ffmpeg at new position
                                }
                                _ => {}
                            }
                        }
                        break 'frame_loop; // Restart ffmpeg after any seeking
                    }
                }
                Ok(VideoControl::FrameForward) => {
                    // Force pause and move forward one frame
                    is_paused = true;
                    show_frame_after_seek = true;
                    pause_start_time = Some(Instant::now());
                    current_time += 1.0 / fps;
                    if current_time >= duration {
                        current_time = duration - (1.0 / fps); // Stay on last frame
                    }
                    let _ = ffmpeg_child.kill();
                    break 'frame_loop; // Restart ffmpeg at new position
                }
                Ok(VideoControl::FrameBackward) => {
                    // Force pause and move backward one frame
                    is_paused = true;
                    show_frame_after_seek = true;
                    pause_start_time = Some(Instant::now());
                    current_time -= 1.0 / fps;
                    current_time = current_time.max(0.0);
                    let _ = ffmpeg_child.kill();
                    break 'frame_loop; // Restart ffmpeg at new position
                }
                Ok(VideoControl::SeekForward(seconds)) => {
                    // Force pause and seek forward
                    is_paused = true;
                    show_frame_after_seek = true;
                    pause_start_time = Some(Instant::now());
                    current_time += seconds;
                    if current_time >= duration {
                        current_time = duration - (1.0 / fps); // Stay on last frame
                    }
                    let _ = ffmpeg_child.kill();
                    break 'frame_loop; // Restart ffmpeg at new position
                }
                Ok(VideoControl::SeekBackward(seconds)) => {
                    // Force pause and seek backward
                    is_paused = true;
                    show_frame_after_seek = true;
                    pause_start_time = Some(Instant::now());
                    current_time -= seconds;
                    current_time = current_time.max(0.0);
                    let _ = ffmpeg_child.kill();
                    break 'frame_loop; // Restart ffmpeg at new position
                }
                _ => {}
            }

            // If paused, check if we need to show one frame after seeking
            if is_paused && !show_frame_after_seek {
                thread::sleep(Duration::from_millis(50));
                continue;
            }

            // Calculate timing based on real elapsed time since current playback segment started
            let elapsed_since_segment_start = playback_start_time.elapsed() - total_paused_duration;
            let expected_video_time =
                playback_start_video_time + elapsed_since_segment_start.as_secs_f64();

            // Handle frame timing and dropping
            let (new_time, should_continue) = handle_frame_timing(
                current_time,
                expected_video_time,
                fps,
                frame_size,
                &mut reader,
                args,
                show_frame_after_seek,
            )?;
            current_time = new_time;
            if should_continue {
                continue;
            }

            // Try to read next frame
            match reader.read_exact(&mut frame_buffer) {
                Ok(()) => {
                    // Successfully read a frame

                    // Convert RGB data to image
                    if let Ok(img) = rgb_data_to_image(&frame_buffer, video_width, video_height) {
                        process_frame_display(
                            img,
                            args,
                            current_time,
                            duration,
                            fps,
                            &mut frames_since_clear,
                            &mut first_frame,
                            &mut previous_terminal_size,
                        )?;

                        // Adaptive FPS monitoring
                        update_adaptive_fps(
                            args,
                            &mut fps,
                            original_fps,
                            &mut consecutive_late_frames,
                            &mut adaptive_fps_active,
                            &mut last_display_time,
                        );

                        // If we just showed a frame after seeking, reset the flag and continue to pause
                        if show_frame_after_seek {
                            show_frame_after_seek = false;
                            // Don't advance time or continue playback - just display this one frame
                            if is_paused {
                                continue; // Go back to input checking without advancing frame
                            }
                        }

                        // Advance to next frame (only for normal playback, not after seeking)
                        current_time += 1.0 / fps;

                        // Break if we've reached the end of the video and we're not paused
                        if current_time >= duration && !is_paused {
                            break 'main_loop; // Natural end of video, exit completely
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        // Normal end of stream from ffmpeg
                        if !is_paused {
                            break 'main_loop; // Natural end of video, exit completely
                        }
                        break 'frame_loop;
                    } else if e.kind() != io::ErrorKind::Interrupted {
                        // Actual error reading from ffmpeg
                        let _ = ffmpeg_child.kill();
                        return Err(anyhow::anyhow!("Error reading frame from ffmpeg: {}", e));
                    }
                }
            }
        }

        // Wait for ffmpeg to finish cleanly
        let _ = ffmpeg_child.wait();
    }

    Ok(())
}

fn rgb_data_to_image(rgb_data: &[u8], width: u32, height: u32) -> Result<DynamicImage> {
    // Verify the data size matches expected dimensions
    let expected_size = (width * height * 3) as usize;
    if rgb_data.len() != expected_size {
        anyhow::bail!(
            "RGB data size mismatch: expected {} bytes, got {} bytes",
            expected_size,
            rgb_data.len()
        );
    }

    // Create an RGB image from the raw data
    let rgb_image = image::RgbImage::from_raw(width, height, rgb_data.to_vec())
        .context("Failed to create RGB image from raw data")?;

    Ok(DynamicImage::ImageRgb8(rgb_image))
}

fn get_video_dimensions(file_path: &Path) -> Result<(u32, u32)> {
    ensure_ffprobe_available()?;

    // Use ffprobe to get video dimensions
    let output = std::process::Command::new("ffprobe")
        .args([
            // `error` (not `quiet`) so a genuinely unprobeable file still emits a
            // real diagnostic on stderr instead of an empty failure message.
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            // One value per line. Unlike `csv=p=0`, this writer does not append
            // an empty trailing field for a stream's side-data section (e.g. a
            // Dolby Vision configuration record), so the output stays clean.
            "default=noprint_wrappers=1:nokey=1",
            file_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffprobe to get video dimensions")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed to get video dimensions: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let dimensions_str = String::from_utf8_lossy(&output.stdout);
    parse_video_dimensions(&dimensions_str)
}

/// Parse video width and height out of ffprobe's `stream=width,height` output.
///
/// ffprobe can emit more than the two values we ask for. When a stream carries
/// side data — for example a Dolby Vision configuration record on a 4K HDR HEVC
/// stream — its CSV writer appends an empty trailing field, yielding output like
/// `"3840,2160,"`. Rather than assume an exact `W,H` layout, we take the first
/// two integer tokens, tolerating commas, whitespace, and newlines so the parse
/// stays correct across ffprobe output formats.
fn parse_video_dimensions(output: &str) -> Result<(u32, u32)> {
    let mut tokens = output
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty());

    let width: u32 = tokens
        .next()
        .context("ffprobe returned no video width")?
        .parse()
        .context("Failed to parse video width")?;
    let height: u32 = tokens
        .next()
        .context("ffprobe returned no video height")?
        .parse()
        .context("Failed to parse video height")?;

    Ok((width, height))
}

/// Frame rate used when ffprobe is unavailable or its output is unparseable.
const DEFAULT_FPS: f64 = 24.0;

fn get_video_fps(file_path: &Path) -> Result<f64> {
    if ensure_ffprobe_available().is_err() {
        eprintln!("Warning: ffprobe not found, using default 24 fps");
        return Ok(DEFAULT_FPS);
    }

    // Use ffprobe to get both frame-rate fields. avg_frame_rate is the true
    // average rate (total frames / duration) and is what playback timing must
    // use; r_frame_rate is only the timebase tick and, for variable-frame-rate
    // recordings, can be a large multiple of the real rate. We keep the keys
    // (no `nokey=1`) because ffprobe emits these fields in its own internal
    // order regardless of the requested order, so position alone is ambiguous.
    let output = std::process::Command::new("ffprobe")
        .args([
            // `error` (not `quiet`) so a genuinely unprobeable file still emits a
            // real diagnostic on stderr instead of an empty failure message.
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=avg_frame_rate,r_frame_rate",
            "-of",
            "default=noprint_wrappers=1",
            file_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        return Ok(DEFAULT_FPS); // Default to 24 fps
    }

    let fps_str = String::from_utf8_lossy(&output.stdout);
    Ok(parse_video_fps(&fps_str))
}

/// Choose a playback frame rate from ffprobe's `stream=avg_frame_rate,r_frame_rate`
/// output (keyed `default` format, one `key=value` per line).
///
/// `avg_frame_rate` (total frames ÷ duration) is the true playback rate and is
/// preferred. For variable-frame-rate recordings — screen captures, for example —
/// `r_frame_rate` is only the timebase tick and can be a large multiple of the
/// real rate, which would make playback run too fast. We fall back to
/// `r_frame_rate` only when `avg_frame_rate` is unknown (ffprobe reports `"0/0"`),
/// and to [`DEFAULT_FPS`] when neither field is usable.
fn parse_video_fps(output: &str) -> f64 {
    let mut avg = None;
    let mut r = None;

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "avg_frame_rate" => avg = parse_fps_fraction(value),
            "r_frame_rate" => r = parse_fps_fraction(value),
            _ => {}
        }
    }

    avg.or(r).unwrap_or(DEFAULT_FPS)
}

/// Parse a single frame-rate field such as `"24000/1001"` into frames per second.
///
/// ffprobe reports rates as a `numerator/denominator` fraction. A stream's side
/// data (e.g. a Dolby Vision configuration record) can make some output formats
/// append an empty trailing field — `"24000/1001,"` — so we take the first
/// whitespace/comma-delimited token before splitting the fraction. Returns
/// [`None`] for an unknown rate (`"0/0"`), an empty field, or any value that does
/// not parse to a finite, positive rate, so the caller can fall back rather than
/// using NaN or a nonsensical rate.
fn parse_fps_fraction(value: &str) -> Option<f64> {
    let token = value
        .split(|c: char| c == ',' || c.is_whitespace())
        .find(|token| !token.is_empty())?;

    // Parse fraction like "24/1" or "30000/1001"; the token is ASCII in practice.
    let fps = if let Some((num_str, denom_str)) = token.split_once('/') {
        let numerator: f64 = num_str.parse().ok()?;
        let denominator: f64 = denom_str.parse().ok()?;
        numerator / denominator
    } else {
        token.parse().ok()?
    };

    (fps.is_finite() && fps > 0.0).then_some(fps)
}

fn get_video_duration(file_path: &Path) -> Result<f64> {
    if ensure_ffprobe_available().is_err() {
        eprintln!("Warning: ffprobe not found, using default duration");
        return Ok(60.0); // Default to 60 seconds
    }

    // Use ffprobe to get video duration in seconds
    let output = std::process::Command::new("ffprobe")
        .args([
            // `error` (not `quiet`) so a genuinely unprobeable file still emits a
            // real diagnostic on stderr instead of an empty failure message.
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            file_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        return Ok(60.0); // Default to 60 seconds
    }

    let duration_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(duration_str.parse().unwrap_or(60.0))
}

fn clear_screen() -> Result<()> {
    // Use more efficient screen clearing for video playback
    print!("\x1b[2J\x1b[H"); // Clear screen and move cursor to top-left
    io::stdout().flush().context("Failed to flush output")?;
    Ok(())
}

fn clear_scrollback() -> Result<()> {
    // Clear iTerm2 scrollback buffer to free memory - for video playback
    print!("\x1b]1337;ClearScrollback\x07");
    io::stdout()
        .flush()
        .context("Failed to flush scrollback clear")?;
    Ok(())
}

fn move_cursor_home() -> Result<()> {
    // Optimized cursor positioning for video frames
    print!("\x1b[1;1H"); // Move cursor to top-left without clearing
    io::stdout().flush().context("Failed to flush output")?;
    Ok(())
}

fn draw_progress_bar(
    current_time: f64,
    total_duration: f64,
    fps: f64,
    terminal_width: u32,
    terminal_height: u32,
) -> Result<()> {
    // Move cursor to bottom of screen
    print!("\x1b[{};1H", terminal_height);

    // Calculate progress
    let progress = if total_duration > 0.0 {
        (current_time / total_duration).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Format time strings with frame numbers.
    // Convert f64 components to u32 via a clamping helper so values that are
    // negative (shouldn't happen but defend against ffprobe surprises) or
    // absurdly large get pinned to the u32 range without panicking.
    let f64_to_u32_clamped = |v: f64| -> u32 {
        if v.is_nan() || v <= 0.0 {
            0
        } else if v >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            // SAFETY of cast: v is finite and in [0.0, u32::MAX), so truncation
            // toward zero produces a representable u32.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "value is bounded to [0, u32::MAX) above"
            )]
            let out = v as u32;
            out
        }
    };

    let current_min = f64_to_u32_clamped(current_time / 60.0);
    let current_sec = f64_to_u32_clamped(current_time % 60.0);
    let current_frame = f64_to_u32_clamped((current_time % 1.0) * fps);

    let total_min = f64_to_u32_clamped(total_duration / 60.0);
    let total_sec = f64_to_u32_clamped(total_duration % 60.0);
    let total_frame = f64_to_u32_clamped((total_duration % 1.0) * fps);

    let time_str = format!(
        "{:02}:{:02}:{:02} / {:02}:{:02}:{:02}",
        current_min, current_sec, current_frame, total_min, total_sec, total_frame
    );

    // Calculate available width for progress bar (leave space for time and brackets).
    // time_str length here is a tiny ASCII string ("MM:SS:FF / MM:SS:FF" ~= 19 chars),
    // far below u32::MAX, so the conversion cannot truncate.
    let time_len = u32::try_from(time_str.len()).unwrap_or(u32::MAX);
    let available_width = terminal_width.saturating_sub(time_len + 4); // 4 chars for " [" and "] "

    if available_width > 0 {
        // progress is in [0.0, 1.0] (clamped above) and available_width is u32,
        // so `progress * available_width as f64` is in [0.0, u32::MAX as f64].
        let filled_chars = f64_to_u32_clamped(progress * f64::from(available_width));
        let empty_chars = available_width - filled_chars;

        // Draw the progress bar
        print!("\x1b[2K"); // Clear the line
        print!("{} [", time_str);
        for _ in 0..filled_chars {
            print!("█");
        }
        for _ in 0..empty_chars {
            print!("░");
        }
        print!("]");
    } else {
        // Terminal too narrow, just show time
        print!("\x1b[2K{}", time_str);
    }

    io::stdout()
        .flush()
        .context("Failed to flush progress bar")?;
    Ok(())
}

fn get_terminal_size() -> Result<(u32, u32)> {
    if let Some((Width(w), Height(h))) = terminal_size() {
        Ok((w as u32, h as u32))
    } else {
        // Fallback to common terminal size if detection fails
        Ok((80, 24))
    }
}

/// Print a header and then display an image file.
///
/// The function prints the header itself and then counts the lines that it
/// printed. The count and the print can therefore never drift apart, so a
/// caller cannot print a header and forget to pay for the rows.
///
/// # Arguments
/// * `file_path` - The path of the image file.
/// * `args` - The command line arguments.
/// * `header` - The lines to print above the image, one line per element.
///
/// # Returns
/// An error when the file does not open as an image, or when the display fails.
fn display_image_from_file(file_path: &Path, args: &Args, header: &[String]) -> Result<()> {
    for line in header {
        println!("{line}");
    }

    let img = image::open(file_path)
        .with_context(|| format!("Failed to open image file: {}", file_path.display()))?;

    let header_rows = u32::try_from(header.len()).unwrap_or(u32::MAX);
    display_image(img, args, args.no_newline, HeaderRows(header_rows))
}

fn display_text_file(file_path: &Path) -> Result<()> {
    let contents = fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read text file: {}", file_path.display()))?;

    print!("{}", contents);
    io::stdout().flush().context("Failed to flush output")?;

    Ok(())
}

fn display_image_from_stdin(args: &Args) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut buffer = Vec::new();
    reader
        .read_to_end(&mut buffer)
        .context("Failed to read image data from stdin")?;

    let img = image::load_from_memory(&buffer).context("Failed to decode image from stdin")?;

    // This path prints no header, so the image can use the whole terminal less
    // the row of the prompt.
    display_image(img, args, args.no_newline, HeaderRows(0))
}

/// Display an image in the terminal.
///
/// # Arguments
/// * `img` - The image to display.
/// * `args` - The command line arguments.
/// * `no_newline` - True when the caller controls the position of the cursor.
/// * `header` - The number of rows that the caller printed above the image. The
///   auto-fit path subtracts these rows from the height of the terminal.
///
/// # Returns
/// An error when the terminal cannot show graphics, or when the display fails.
fn display_image(
    img: DynamicImage,
    args: &Args,
    no_newline: bool,
    header: HeaderRows,
) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    let transport = detect_remote_transport();

    validate_terminal_for_graphics(&terminal_caps, &transport, "Image")?;

    // Always use character-based sizing (fit mode), but respect user-specified dimensions if provided
    let (target_width, target_height) = if args.width.is_some() || args.height.is_some() {
        // Use user-specified dimensions but still use character-based sizing
        (args.width, args.height)
    } else {
        // Auto-fit to terminal window size
        let (term_width, term_height) = get_terminal_size()?;
        // Leave some margin for terminal chrome
        let safe_width = if term_width > 4 {
            term_width - 2
        } else {
            term_width
        };
        // The header rows and the prompt row come out of the height, so the
        // header, the image and the prompt fit inside the terminal.
        let safe_height = auto_fit_rows(term_height, header);
        (Some(safe_width), Some(safe_height))
    };

    // Apply scaling.
    // args.scale is u8 in [0, 100); scale_factor is therefore in [0.0, 1.0),
    // so `w as f32 * scale_factor` is non-negative and <= w, and a u32 input
    // always fits in f32's value range (with possible precision loss for very
    // large widths, which is acceptable for display scaling).
    let (scaled_width, scaled_height) = if args.scale < 100 {
        let scale_factor = f32::from(args.scale) / 100.0;
        let scale_dim = |dim: u32| -> u32 {
            // Cast u32 -> f32 can lose precision for values above 2^24, but
            // that's fine for image dimensions in this codebase.
            #[allow(
                clippy::cast_precision_loss,
                reason = "image dimensions rarely exceed 2^24"
            )]
            let dim_f = dim as f32 * scale_factor;
            // dim_f is finite and in [0.0, dim as f32], so it fits in u32.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "dim_f is non-negative and bounded by the original u32 dimension"
            )]
            let scaled = dim_f as u32;
            scaled.max(1)
        };
        (target_width.map(scale_dim), target_height.map(scale_dim))
    } else {
        (target_width, target_height)
    };

    // Choose optimal display method based on terminal capabilities. The routing
    // is resolved by display_routine_for(), which is exhaustive over terminal
    // types so a new variant can never silently fall through to the wrong
    // protocol. Use already-detected terminal_caps to avoid redundant env lookups.
    //
    // No routine needs a special path for a remote proxy. Every routine holds
    // the renderer still and then states the position of the cursor itself, so
    // a remote proxy tracks the cursor with the same newlines, CUU and CUD that
    // a local terminal does.
    match display_routine_for(&terminal_caps.terminal_type) {
        DisplayRoutine::Sixel => {
            display_image_sixel(&img, scaled_width, scaled_height, args, no_newline)
        }
        DisplayRoutine::Kitty => {
            display_image_kitty(&img, scaled_width, scaled_height, args, no_newline)
        }
        DisplayRoutine::Iterm2 => {
            display_image_iterm2(&img, scaled_width, scaled_height, args, no_newline)
        }
    }
}

/// Calculate display dimensions that preserve aspect ratio within the given bounds.
///
/// Terminal cells are typically ~2:1 (height:width in pixels), so we account for that
/// via the `cell_aspect` parameter (cell height / cell width in pixels). Callers
/// obtain this from `get_cell_pixel_dimensions()`, which uses actual terminal
/// dimensions when available and falls back to
/// `ESTIMATED_CELL_HEIGHT_PX / ESTIMATED_CELL_WIDTH_PX`. This function works in
/// terminal character cells, not pixels.
///
/// The casts from f64 to u32 are intentional - display dimensions are always positive
/// and will never exceed u32::MAX for any reasonable terminal size.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "display dimensions are always positive and fit in u32"
)]
fn calculate_aspect_preserving_size(
    img_width: u32,
    img_height: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    preserve_aspect: bool,
    cell_aspect: f64,
) -> (Option<u32>, Option<u32>) {
    if !preserve_aspect {
        return (max_width, max_height);
    }

    // Guard against division by zero. Valid images always have height > 0,
    // but we handle this defensively to avoid panics on malformed input.
    if img_height == 0 {
        return (max_width, max_height);
    }

    match (max_width, max_height) {
        (Some(max_w), Some(max_h)) => {
            let effective_max_h_pixels = max_h as f64 * cell_aspect;
            let effective_max_w_pixels = max_w as f64;

            let img_aspect = img_width as f64 / img_height as f64;
            let box_aspect = effective_max_w_pixels / effective_max_h_pixels;

            if img_aspect > box_aspect {
                // Image is wider than box - constrain by width
                let display_width = max_w;
                let display_height = ((max_w as f64 / img_aspect) / cell_aspect).round() as u32;
                (Some(display_width), Some(display_height.max(1)))
            } else {
                // Image is taller than box - constrain by height
                let display_height = max_h;
                let display_width = (max_h as f64 * cell_aspect * img_aspect).round() as u32;
                (Some(display_width.max(1)), Some(display_height))
            }
        }
        (Some(w), None) => (Some(w), None),
        (None, Some(h)) => (None, Some(h)),
        (None, None) => (None, None),
    }
}

/// Calculate the pixel dimensions of a single terminal character cell.
///
/// Tries to determine actual cell dimensions via ioctl (TIOCGWINSZ). Falls back to
/// estimated constants if the ioctl fails or the terminal reports zero dimensions.
fn get_cell_pixel_dimensions() -> (u32, u32) {
    if let Some((total_px_w, total_px_h)) = get_terminal_pixel_size() {
        let (term_cols, term_rows) = get_terminal_size().unwrap_or((80, 24));
        if term_cols > 0 && term_rows > 0 {
            return (total_px_w / term_cols, total_px_h / term_rows);
        }
    }
    (ESTIMATED_CELL_WIDTH_PX, ESTIMATED_CELL_HEIGHT_PX)
}

/// Returns the cell aspect ratio (height / width in pixels).
/// Uses actual terminal cell dimensions when available, falling back to estimates.
fn get_cell_aspect_ratio() -> f64 {
    let (cell_w, cell_h) = get_cell_pixel_dimensions();
    cell_h as f64 / cell_w as f64
}

/// Convert character cell display dimensions to target pixel dimensions.
///
/// Returns `None` if either dimension is missing (meaning no downscale target can
/// be computed). Uses actual terminal cell pixel dimensions when available, falling
/// back to estimated constants.
fn cells_to_pixels(cols: u32, rows: u32, cell_w: u32, cell_h: u32) -> (u32, u32) {
    (cols * cell_w, rows * cell_h)
}

/// Downscale an image to fit the display pixel dimensions if it exceeds them.
///
/// Converts character cell display dimensions to pixel dimensions using either the
/// actual terminal pixel size (via ioctl) or estimated cell dimensions, then resizes
/// the image if it's larger than the target. This prevents sending hundreds of megabytes
/// of pixel data to the terminal for very large images (e.g., panoramas).
///
/// Returns a borrowed reference to the original image when no downscaling is needed
/// (dimensions unspecified or image already fits), avoiding an unnecessary clone.
/// Returns an owned downscaled image when the original exceeds the target pixel dimensions.
fn downscale_to_display_pixels<'a>(
    img: &'a DynamicImage,
    display_width: Option<u32>,
    display_height: Option<u32>,
) -> Cow<'a, DynamicImage> {
    let (target_pixel_w, target_pixel_h) = match (display_width, display_height) {
        (Some(cols), Some(rows)) => {
            let (cell_w, cell_h) = get_cell_pixel_dimensions();
            cells_to_pixels(cols, rows, cell_w, cell_h)
        }
        _ => return Cow::Borrowed(img),
    };

    if img.width() <= target_pixel_w && img.height() <= target_pixel_h {
        return Cow::Borrowed(img);
    }

    Cow::Owned(img.resize(
        target_pixel_w,
        target_pixel_h,
        image::imageops::FilterType::Lanczos3,
    ))
}

/// Kitty terminal display with better performance for video.
///
/// Also used for WezTerm and Ghostty, which support the Kitty graphics protocol.
fn display_image_kitty(
    img: &DynamicImage,
    width: Option<u32>,
    height: Option<u32>,
    args: &Args,
    no_newline: bool,
) -> Result<()> {
    // display_width/display_height are in terminal cells (character columns/rows).
    // They serve two roles: (1) the Kitty protocol `c=`/`r=` display hints that tell
    // the terminal how many cells the image should span, and (2) the downscale target
    // converted to pixels via cell dimensions to cap the raw data we send.
    let (display_width, display_height) = calculate_aspect_preserving_size(
        img.width(),
        img.height(),
        width,
        height,
        args.preserve_aspect,
        get_cell_aspect_ratio(),
    );

    // Downscale the image to the target pixel size before sending to avoid
    // overwhelming the terminal with huge payloads (e.g., a 16384x8192 panorama
    // would produce ~384MB of RGB data before base64 encoding).
    let img = downscale_to_display_pixels(img, display_width, display_height);

    // Use RGB data directly for better performance (no base64 encoding overhead)
    let rgb_data = img.to_rgb8();

    // Use Kitty's more efficient graphics protocol with optimizations
    print_kitty_image(
        rgb_data.as_raw(),
        img.width(),
        img.height(),
        display_width,
        display_height,
        no_newline,
    )
}

/// Get terminal pixel dimensions using ioctl if available.
///
/// Uses the TIOCGWINSZ ioctl to query the terminal's pixel dimensions.
/// Returns None if the ioctl fails or reports zero dimensions.
fn get_terminal_pixel_size() -> Option<(u32, u32)> {
    use std::os::unix::io::AsRawFd;

    #[repr(C)]
    struct Winsize {
        ws_row: libc::c_ushort,
        ws_col: libc::c_ushort,
        ws_xpixel: libc::c_ushort,
        ws_ypixel: libc::c_ushort,
    }

    let mut ws = Winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let fd = io::stdout().as_raw_fd();
    // SAFETY: TIOCGWINSZ is a standard ioctl that only reads the terminal window size
    // into the provided Winsize struct. It does not modify any other state and the
    // Winsize struct is properly initialized.
    let result = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };

    if result == 0 && ws.ws_xpixel > 0 && ws.ws_ypixel > 0 {
        Some((ws.ws_xpixel as u32, ws.ws_ypixel as u32))
    } else {
        None
    }
}

/// Calculate pixel dimensions for Sixel output, preserving aspect ratio.
///
/// Unlike `calculate_aspect_preserving_size` which works in terminal character cells,
/// this function works directly in pixels for Sixel output. The aspect ratio calculation
/// is intentionally different because Sixel doesn't need to account for cell aspect ratio.
///
/// The casts from f64 to u32 are intentional - pixel dimensions are always positive
/// and will never exceed u32::MAX for any reasonable display.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel dimensions are always positive and fit in u32"
)]
fn calculate_sixel_dimensions(
    img_width: u32,
    img_height: u32,
    target_pixel_width: u32,
    target_pixel_height: u32,
    preserve_aspect: bool,
) -> (u32, u32) {
    if !preserve_aspect {
        return (target_pixel_width, target_pixel_height);
    }

    let img_aspect = img_width as f64 / img_height as f64;
    let target_aspect = target_pixel_width as f64 / target_pixel_height as f64;

    if img_aspect > target_aspect {
        // Image is wider - constrain by width
        let w = target_pixel_width;
        let h = (target_pixel_width as f64 / img_aspect) as u32;
        (w, h.max(1))
    } else {
        // Image is taller - constrain by height
        let h = target_pixel_height;
        let w = (target_pixel_height as f64 * img_aspect) as u32;
        (w.max(1), h)
    }
}

/// Calculate the number of terminal rows that an image fills.
///
/// The image height in pixels divides by the height of one character cell, and
/// the result rounds up, because a partial row still uses a full row.
///
/// # Arguments
/// * `height_px` - The height of the image in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The row count. The result is always 1 or more, so the caller never asks the
/// terminal for a movement of zero rows. A `cell_height_px` of 0 counts as 1,
/// because `get_cell_pixel_dimensions` divides the terminal pixel height by the
/// row count and can give 0.
fn image_rows(height_px: u32, cell_height_px: u32) -> u32 {
    height_px.div_ceil(cell_height_px.max(1)).max(1)
}

/// Calculate the number of terminal rows that a rendered image fills, for the
/// protocols that take the size of the image in character cells.
///
/// The Kitty graphics protocol and the iTerm2 inline image protocol both take a
/// width and a height in character cells, and both make their own decision when
/// one of the two is absent. There are four cases, and each one follows a rule
/// of the protocols:
///
/// * `display_height` is `Some(h)`, with or without a width. Both protocols
///   scale the image into the rows that `ic` asks for, so the answer is `h`.
/// * Only `display_width` is `Some(w)`. Both protocols then compute the other
///   dimension to keep the aspect ratio of the image. The rendered height in
///   pixels is `img_height_px * (w * cell_width_px) / img_width_px`.
/// * Neither is given. Both protocols render the image at its own pixel size,
///   so the answer comes from the pixel height of the image.
///
/// # Arguments
/// * `img_width_px` - The width of the image in pixels.
/// * `img_height_px` - The height of the image in pixels.
/// * `display_width` - The width that `ic` asks for, in character cells.
/// * `display_height` - The height that `ic` asks for, in character cells.
/// * `cell_width_px` - The width of one character cell in pixels.
/// * `cell_height_px` - The height of one character cell in pixels.
///
/// # Returns
/// The row count. The result is always 1 or more, so the caller never asks the
/// terminal for a movement of zero rows, which a terminal reads as one row. An
/// `img_width_px` of 0 gives no scale factor, so the width-only case falls back
/// to the pixel height of the image.
fn image_rows_in_cells(
    img_width_px: u32,
    img_height_px: u32,
    display_width: Option<u32>,
    display_height: Option<u32>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> u32 {
    if let Some(rows) = display_height {
        return rows.max(1);
    }

    match display_width {
        // The arithmetic runs in u64, because a wide image and a tall image
        // together overflow u32 long before they overflow u64.
        Some(cols) if img_width_px > 0 => {
            let rendered_height_px =
                u64::from(img_height_px) * u64::from(cols) * u64::from(cell_width_px)
                    / u64::from(img_width_px);
            image_rows(
                u32::try_from(rendered_height_px).unwrap_or(u32::MAX),
                cell_height_px,
            )
        }
        _ => image_rows(img_height_px, cell_height_px),
    }
}

/// Where a display routine must leave the cursor after it writes an image.
///
/// Sixel gives no contract for the position of the cursor after the string
/// terminator, and each renderer decides for itself. The Kitty protocol and the
/// iTerm2 protocol each have a flag that holds the cursor still, but then the
/// caller must move it. `ic` therefore states the position of the cursor
/// instead of a guess.
enum CursorContract {
    /// The caller puts the cursor where it wants it (`--no-newline`, video
    /// playback). The routine writes the payload and nothing else.
    CallerManaged,
    /// Column 1 of the first row below the image.
    BelowImage {
        /// The height of the image in terminal rows.
        rows: u32,
    },
}

/// Write an image payload and keep the promise of a [`CursorContract`].
///
/// Every display routine of `ic` owes the caller the same promise: the cursor
/// ends at column 1 of the first row below the image, unless the caller asked
/// for no newline. This function holds the one implementation of that promise,
/// so a new display routine cannot forget it.
///
/// [`CursorContract::BelowImage`] writes four parts:
///
/// 1. One newline for each row of the image. The reservation makes an image at
///    the bottom of the screen scroll the terminal instead of run off it.
/// 2. CUU, which goes back to the top of the reservation.
/// 3. DECSC, the payload and DECRC. The brackets make sure that cursor motion
///    inside the payload cannot change the final position of the cursor.
/// 4. CUD by the row count and a carriage return, which together put the cursor
///    at column 1 of the first row below the image.
///
/// The payload arrives as a closure, because the Kitty routine writes its
/// payload in more than one chunk and cannot make one string.
///
/// # Arguments
/// * `out` - The stream that takes the bytes.
/// * `contract` - The promise that the routine must keep.
/// * `payload` - A closure that writes the image payload to `out`.
///
/// # Returns
/// An error when a write to `out` fails.
fn write_image_with_cursor_contract<W, F>(
    out: &mut W,
    contract: CursorContract,
    payload: F,
) -> io::Result<()>
where
    W: Write,
    F: FnOnce(&mut W) -> io::Result<()>,
{
    let CursorContract::BelowImage { rows } = contract else {
        return payload(out);
    };

    // The row count is always 1 or more, so this never asks the terminal for a
    // movement of zero rows.
    for _ in 0..rows {
        writeln!(out)?;
    }
    write!(out, "{CSI}{rows}A")?;

    write!(out, "{SAVE_CURSOR}")?;
    payload(out)?;
    write!(out, "{RESTORE_CURSOR}")?;

    write!(out, "{CSI}{rows}B\r")
}

/// Sixel display for terminals that support Sixel graphics (e.g., Zellij passthrough).
///
/// # Arguments
/// * `img` - The image to display
/// * `fallback_cols` - Fallback terminal width in character cells (used if pixel size unavailable)
/// * `fallback_rows` - Fallback terminal height in character cells (used if pixel size unavailable)
/// * `args` - Command line arguments
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pixel dimensions are always positive and fit in u32"
)]
fn display_image_sixel(
    img: &DynamicImage,
    fallback_cols: Option<u32>,
    fallback_rows: Option<u32>,
    args: &Args,
    no_newline: bool,
) -> Result<()> {
    // Try to get actual terminal pixel dimensions, with fallbacks
    let (target_pixel_width, target_pixel_height) =
        if let Some((px_w, px_h)) = get_terminal_pixel_size() {
            // Leave some margin to avoid overflow
            (
                (px_w as f64 * SIXEL_HORIZONTAL_MARGIN) as u32,
                (px_h as f64 * SIXEL_VERTICAL_MARGIN) as u32,
            )
        } else if let (Some(cols), Some(rows)) = (fallback_cols, fallback_rows) {
            // Fall back to character cell estimates using typical cell dimensions
            (
                cols * ESTIMATED_CELL_WIDTH_PX,
                rows * ESTIMATED_CELL_HEIGHT_PX,
            )
        } else {
            // Default to reasonable size when no size info available
            (DEFAULT_SIXEL_WIDTH_PX, DEFAULT_SIXEL_HEIGHT_PX)
        };

    // Calculate dimensions that preserve aspect ratio
    let (final_width, final_height) = calculate_sixel_dimensions(
        img.width(),
        img.height(),
        target_pixel_width,
        target_pixel_height,
        args.preserve_aspect,
    );

    // Resize image to fit (scale up or down as needed)
    let resized_img = img.resize_exact(
        final_width,
        final_height,
        image::imageops::FilterType::Lanczos3,
    );

    // Convert to RGBA for sixel encoding
    let rgba_img = resized_img.to_rgba8();
    let rgba_data = rgba_img.as_raw();

    // Encode to Sixel format using high-quality settings
    let sixel_output = sixel_encode(
        rgba_data,
        resized_img.width() as usize,
        resized_img.height() as usize,
        &EncodeOptions::default(),
    )
    .map_err(|e| anyhow::anyhow!("Sixel encoding failed: {e}"))?;

    // Output the sixel data
    let mut stdout = io::stdout().lock();

    let contract = if no_newline {
        CursorContract::CallerManaged
    } else {
        let (_, cell_height_px) = get_cell_pixel_dimensions();
        CursorContract::BelowImage {
            rows: image_rows(resized_img.height(), cell_height_px),
        }
    };

    write_image_with_cursor_contract(&mut stdout, contract, |out| write!(out, "{}", sixel_output))?;

    stdout.flush().context("Failed to flush output")?;

    Ok(())
}

/// Optimized iTerm2 display with reduced overhead.
///
/// Computes aspect-preserving display dimensions and downscales the image to the
/// target pixel size before encoding. This matches the Kitty path's behavior and
/// prevents sending oversized payloads for very large images.
fn display_image_iterm2(
    img: &DynamicImage,
    width: Option<u32>,
    height: Option<u32>,
    args: &Args,
    no_newline: bool,
) -> Result<()> {
    // Calculate aspect-corrected display dimensions in terminal cells, then use them
    // both as the iTerm2 width/height hints and the downscale target.
    let (display_width, display_height) = calculate_aspect_preserving_size(
        img.width(),
        img.height(),
        width,
        height,
        args.preserve_aspect,
        get_cell_aspect_ratio(),
    );

    // Downscale the image to the target pixel size before encoding to avoid
    // overwhelming the terminal with huge payloads
    let img = downscale_to_display_pixels(img, display_width, display_height);

    // Use more efficient encoding for iTerm2
    // Convert to RGB first for consistency and smaller data size than RGBA
    let rgb_img = img.to_rgb8();
    let rgb_data = rgb_img.as_raw();

    // Create PNM header manually for more control
    let pnm_header = format!("P6\n{} {}\n255\n", img.width(), img.height());
    let mut pnm_data = Vec::with_capacity(pnm_header.len() + rgb_data.len());
    pnm_data.extend_from_slice(pnm_header.as_bytes());
    pnm_data.extend_from_slice(rgb_data);

    // Use base64 encoding
    let encoded = BASE64_STANDARD.encode(&pnm_data);

    print_iterm2_image(
        &encoded,
        img.width(),
        img.height(),
        display_width,
        display_height,
        no_newline,
    )
}

/// An inline-image protocol that `ic` can emit into a muxiavelli panel.
///
/// muxiavelli panels render through ttyd's xterm.js with `@xterm/addon-image`,
/// which supports **Sixel and iTerm2 IIP only**. The Kitty graphics protocol is
/// deliberately absent from this enum: it is structurally impossible for `ic` to
/// pick Kitty for a muxiavelli panel, which is how the shared contract's "Never
/// Kitty" guarantee is enforced at compile time rather than by convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MuxiavelliImageProtocol {
    Sixel,
    Iterm2,
}

impl MuxiavelliImageProtocol {
    /// Parse a single `MUXIAVELLI_IMAGE_PROTOCOLS` token (case- and
    /// whitespace-insensitive). Returns `None` for anything `ic` cannot emit
    /// into a muxiavelli panel (including `kitty`), so unsupported tokens are
    /// simply skipped when selecting from the advertised list.
    fn parse_token(token: &str) -> Option<Self> {
        let token = token.trim();
        if token.eq_ignore_ascii_case("sixel") {
            Some(Self::Sixel)
        } else if token.eq_ignore_ascii_case("iterm2") {
            Some(Self::Iterm2)
        } else {
            None
        }
    }
}

/// The protocol `ic` falls back to for a muxiavelli panel when the advertised
/// preference list is absent, empty, or names nothing `ic` supports.
const DEFAULT_MUXIAVELLI_PROTOCOL: MuxiavelliImageProtocol = MuxiavelliImageProtocol::Sixel;

/// Resolve the ordered `MUXIAVELLI_IMAGE_PROTOCOLS` preference list to the first
/// protocol `ic` can emit, honoring the advertised order rather than hardcoding
/// a choice. Falls back to Sixel (never Kitty) when the list is absent, empty,
/// or names nothing supported.
fn select_muxiavelli_protocol(raw: Option<&str>) -> MuxiavelliImageProtocol {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(MuxiavelliImageProtocol::parse_token)
        .next()
        .unwrap_or(DEFAULT_MUXIAVELLI_PROTOCOL)
}

/// Optimized Kitty image printing with reduced protocol overhead
#[derive(Debug, Clone, PartialEq, Eq)]
enum TerminalType {
    Kitty,
    Ghostty,
    ITerm2,
    WezTerm,
    Alacritty,
    Zellij,
    /// A muxiavelli panel (ttyd/xterm.js). Carries the resolved inline-image
    /// protocol so the host terminal's leaked env vars cannot override it.
    Muxiavelli(MuxiavelliImageProtocol),
    Unknown,
}

#[derive(Debug, Clone)]
struct TerminalCapabilities {
    terminal_type: TerminalType,
    supports_graphics: bool,
    supports_raw_mode: bool,
}

/// Snapshot of the environment variables that influence terminal detection.
///
/// Captured once so that [`classify_terminal_type`] is a pure function of its
/// inputs — testable without mutating process-global env vars (which is `unsafe`
/// in the 2024 edition and not parallel-safe for the test suite).
#[derive(Debug, Clone, Default)]
struct TerminalEnv {
    term: String,
    term_program: String,
    zellij: bool,
    muxiavelli: bool,
    muxiavelli_protocols: Option<String>,
    kitty_window_id: bool,
    ghostty_resources_dir: bool,
    iterm_session_id: bool,
    alacritty_socket: bool,
}

impl TerminalEnv {
    /// Capture the detection-relevant environment variables from the process.
    fn from_process() -> Self {
        let is_set = |key: &str| std::env::var(key).is_ok();
        TerminalEnv {
            term: std::env::var("TERM").unwrap_or_default(),
            term_program: std::env::var("TERM_PROGRAM").unwrap_or_default(),
            zellij: is_set("ZELLIJ"),
            // The contract pins MUXIAVELLI to the literal "1"; treat any other
            // value (including "0") as "not a muxiavelli panel".
            muxiavelli: matches!(std::env::var("MUXIAVELLI").as_deref(), Ok("1")),
            muxiavelli_protocols: std::env::var("MUXIAVELLI_IMAGE_PROTOCOLS").ok(),
            kitty_window_id: is_set("KITTY_WINDOW_ID"),
            ghostty_resources_dir: is_set("GHOSTTY_RESOURCES_DIR"),
            iterm_session_id: is_set("ITERM_SESSION_ID"),
            alacritty_socket: is_set("ALACRITTY_SOCKET"),
        }
    }
}

/// Classify the terminal from a captured [`TerminalEnv`].
///
/// `MUXIAVELLI` is checked **first** — before Zellij and every host-terminal
/// signal — because a muxiavelli panel's PTY inherits (leaks) the env vars of
/// whatever terminal launched the muxiavelli server. Honoring the panel's own
/// advertised capability is the only correct signal; the leaked host vars must
/// not win. (Same reasoning as checking Zellij before the host terminals.)
fn classify_terminal_type(env: &TerminalEnv) -> TerminalType {
    if env.muxiavelli {
        TerminalType::Muxiavelli(select_muxiavelli_protocol(
            env.muxiavelli_protocols.as_deref(),
        ))
    } else if env.zellij {
        TerminalType::Zellij
    } else if env.kitty_window_id || env.term.contains("kitty") {
        TerminalType::Kitty
    } else if env.term_program == "ghostty"
        || env.ghostty_resources_dir
        || env.term.contains("ghostty")
    {
        TerminalType::Ghostty
    } else if env.term_program.contains("iTerm") || env.iterm_session_id {
        TerminalType::ITerm2
    } else if env.term_program.contains("WezTerm") {
        TerminalType::WezTerm
    } else if env.alacritty_socket || env.term.contains("alacritty") {
        TerminalType::Alacritty
    } else {
        TerminalType::Unknown
    }
}

/// Whether a resolved terminal type can render inline graphics at all.
///
/// muxiavelli panels always can (xterm.js `@xterm/addon-image`), regardless of
/// any `TERM` leaked from a backing session.
fn terminal_supports_graphics(terminal_type: &TerminalType, term: &str) -> bool {
    match terminal_type {
        TerminalType::Muxiavelli(_) => true,
        TerminalType::Alacritty => false,
        _ => {
            !term.contains("linux")   // Linux console doesn't support graphics
                && !term.contains("screen") // Screen doesn't support graphics
                && !term.starts_with("vt") // VT terminals don't support graphics
        }
    }
}

/// The concrete inline-image routine a resolved terminal type dispatches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisplayRoutine {
    Sixel,
    Kitty,
    Iterm2,
}

/// Map a resolved [`TerminalType`] to the display routine `ic` should use.
///
/// Deliberately exhaustive (no wildcard arm): adding a future `TerminalType`
/// forces a conscious routing decision here instead of silently falling through
/// to iTerm2 — the exact failure mode that sent the Kitty protocol into
/// muxiavelli panels.
fn display_routine_for(terminal_type: &TerminalType) -> DisplayRoutine {
    match terminal_type {
        // Zellij and muxiavelli-Sixel both go through the Sixel path.
        TerminalType::Zellij | TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel) => {
            DisplayRoutine::Sixel
        }
        TerminalType::Muxiavelli(MuxiavelliImageProtocol::Iterm2) => DisplayRoutine::Iterm2,
        TerminalType::Kitty | TerminalType::Ghostty | TerminalType::WezTerm => {
            DisplayRoutine::Kitty
        }
        TerminalType::ITerm2 | TerminalType::Alacritty | TerminalType::Unknown => {
            DisplayRoutine::Iterm2
        }
    }
}

fn detect_terminal_capabilities() -> TerminalCapabilities {
    let env = TerminalEnv::from_process();
    let terminal_type = classify_terminal_type(&env);
    let supports_graphics = terminal_supports_graphics(&terminal_type, &env.term);

    let supports_raw_mode = {
        use std::os::unix::io::AsRawFd;
        // SAFETY: isatty() is a read-only check that only examines whether the file
        // descriptor refers to a terminal. It has no side effects and cannot cause
        // memory unsafety. The file descriptor from stdout is always valid.
        unsafe { libc::isatty(io::stdout().as_raw_fd()) == 1 }
    };

    TerminalCapabilities {
        terminal_type,
        supports_graphics,
        supports_raw_mode,
    }
}

/// Process ID newtype for type safety in process tree walking.
///
/// Prevents accidentally mixing up pid/ppid values or confusing
/// process IDs with other u32 values (e.g., loop counters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Pid(u32);

/// Maximum depth to walk up the process tree when searching for ancestors.
/// 64 levels is generous; real-world process trees rarely exceed 20 levels.
const MAX_ANCESTOR_DEPTH: usize = 64;

/// The basename of the Zellij client program. The match must be exact, so
/// that `zellij-server` and a wrapper script such as `my-zellij-wrapper` do
/// not count as clients.
const ZELLIJ_PROGRAM_NAME: &str = "zellij";

/// Extracts the basename (filename) from a process comm string.
///
/// On macOS, `ps -eo comm=` returns the full executable path (e.g.,
/// `/usr/local/bin/mosh-server`). This function extracts just the
/// filename component for exact matching.
fn comm_basename(comm: &str) -> &str {
    Path::new(comm)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(comm)
}

/// Finds the Zellij client processes that are attached to one Zellij session.
///
/// Zellij daemonizes its server, so the server reparents to PID 1 and the
/// chain from the current process to the terminal is broken. The client
/// process keeps that chain, so the client stands in for the current process
/// when the transport is detected.
///
/// A client must belong to *this* session. A machine can run many Zellij
/// sessions at the same time, and a client of some other session says nothing
/// about how this session is viewed.
///
/// `ps_args_output` is the output of `ps -eo pid=,args=`. A client is a
/// process whose argv[0] basename is exactly `zellij` and that has the session
/// name as a complete argument. The forms `zellij a NAME`, `zellij attach
/// NAME`, `zellij -s NAME`, and `zellij --session NAME` all match. The server
/// process (`zellij --server /path/.../NAME`) does not match, because the
/// session name is only a part of its socket path.
fn zellij_client_pids(ps_args_output: &str, session: &str) -> Vec<Pid> {
    if session.is_empty() {
        return Vec::new();
    }

    let mut clients = Vec::new();

    for line in ps_args_output.lines() {
        let mut parts = line.split_whitespace();
        let pid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => Pid(p),
            None => continue,
        };
        let Some(program) = parts.next() else {
            continue;
        };
        if comm_basename(program) != ZELLIJ_PROGRAM_NAME {
            continue;
        }
        // The remaining tokens are the arguments of the client. The session
        // name must be one complete argument.
        if parts.any(|arg| arg == session) {
            clients.push(pid);
        }
    }

    clients
}

/// The type of remote transport detected in the process tree.
///
/// Used to adapt image display behavior for proxies that don't understand
/// terminal graphics protocols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteTransport {
    /// No remote transport detected (direct terminal or plain SSH).
    None,
    /// Running under Mosh (mosh-server in process tree).
    /// Mosh strips escape sequences, so graphics protocols cannot work.
    Mosh,
    /// Running under Eternal Terminal (ET_VERSION env var or etterminal in process tree).
    /// ET passes escape sequences through but its virtual terminal doesn't
    /// track cursor movement from image protocols, requiring explicit
    /// cursor advancement.
    EternalTerminal,
}

/// Return the cached remote transport, detecting it on first call.
///
/// The result is cached in a `OnceLock` because the transport cannot change
/// during a process's lifetime and detection spawns a `ps` subprocess — too
/// expensive to repeat per video frame.
fn detect_remote_transport() -> RemoteTransport {
    static TRANSPORT: OnceLock<RemoteTransport> = OnceLock::new();
    *TRANSPORT.get_or_init(detect_remote_transport_inner)
}

/// Detect the remote transport by checking environment variables and process tree.
///
/// Priority logic:
/// 1. **Mosh as direct ancestor** → always Mosh (the shell is definitely under Mosh)
/// 2. **ET detected** (env var or process tree) → EternalTerminal, even if Mosh is
///    also attached to the same Zellij session. In multiplexed sessions, output goes
///    to all clients — ET viewers can handle images while Mosh viewers silently
///    ignore the escape sequences.
/// 3. **Mosh via Zellij heuristic only** (no ET) → Mosh
/// 4. **Neither** → None
fn detect_remote_transport_inner() -> RemoteTransport {
    let comm_output = match run_ps(&["-eo", "pid=,ppid=,comm="]) {
        Some(o) => o,
        None => {
            // Can't check process tree; fall back to env var for ET
            if std::env::var("ET_VERSION").is_ok() {
                return RemoteTransport::EternalTerminal;
            }
            return RemoteTransport::None;
        }
    };

    let zellij_session = std::env::var("ZELLIJ")
        .ok()
        .map(|_| std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default());

    // The argument snapshot is a second `ps` call, because the comm snapshot
    // deliberately treats every token after the PPID as one executable path,
    // so that a path that holds a space survives. Arguments cannot be read
    // from that same line without making the path ambiguous. Only a Zellij
    // session needs the arguments, so a session outside Zellij does not pay
    // for the second call.
    let args_output = if zellij_session.is_some() {
        run_ps(&["-eo", "pid=,args="]).unwrap_or_default()
    } else {
        String::new()
    };

    classify_transport(
        &comm_output,
        &args_output,
        Pid(std::process::id()),
        zellij_session,
        std::env::var("ET_VERSION").is_ok(),
    )
}

/// Run `ps` with the given arguments and return its standard output.
///
/// Returns `None` when `ps` cannot be run at all.
fn run_ps(args: &[&str]) -> Option<String> {
    std::process::Command::new("ps")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Decide the remote transport from two process snapshots and the environment.
///
/// This function holds all of the policy and reads nothing from the outside,
/// so every rule below is testable.
///
/// `zellij_session` is `None` when the `ZELLIJ` variable is unset. It is
/// `Some(name)` inside Zellij, where `name` can be empty if
/// `ZELLIJ_SESSION_NAME` is unset.
fn classify_transport(
    ps_comm_output: &str,
    ps_args_output: &str,
    current_pid: Pid,
    zellij_session: Option<String>,
    et_version_set: bool,
) -> RemoteTransport {
    // Direct Mosh ancestry (Case 1 only, no Zellij heuristic) — the current
    // shell is definitely under Mosh, so images cannot work.
    if find_ancestor_process(ps_comm_output, current_pid, &ZellijScan::Off, "mosh-server") {
        return RemoteTransport::Mosh;
    }

    let scan = match &zellij_session {
        None => ZellijScan::Off,
        Some(session) => match ClientPids::new(zellij_client_pids(ps_args_output, session)) {
            Some(clients) => ZellijScan::Clients(clients),
            // The session named no client, so the careful answer is the one
            // that treats every Zellij client on the machine as a candidate.
            // It can over-report Mosh, which hides an image that would have
            // worked. The opposite mistake writes escape sequences that Mosh
            // strips.
            None => ZellijScan::EveryClient,
        },
    };

    // ET detected via env var or process tree (including Zellij heuristic).
    // This takes priority over Mosh-via-Zellij because in a multiplexed Zellij
    // session, Mosh and ET may both be attached — ET viewers can display images
    // while Mosh viewers silently strip the escape sequences.
    if et_version_set || find_ancestor_process(ps_comm_output, current_pid, &scan, "etterminal") {
        return RemoteTransport::EternalTerminal;
    }

    // Mosh via Zellij heuristic (Case 2) — only reached when ET is not present.
    let mosh = match &scan {
        // The direct walk above already answered this question.
        ZellijScan::Off => false,
        // Zellij sends the output of a pane to every attached client. One
        // client that can show images is enough, so Mosh only blocks when
        // every client of this session is a Mosh client.
        ZellijScan::Clients(clients) => clients.as_slice().iter().all(|&client| {
            find_ancestor_process(ps_comm_output, client, &ZellijScan::Off, "mosh-server")
        }),
        ZellijScan::EveryClient => {
            find_ancestor_process(ps_comm_output, current_pid, &scan, "mosh-server")
        }
    };
    if mosh {
        return RemoteTransport::Mosh;
    }

    RemoteTransport::None
}

/// Holds [`ClientPids`] so that its field stays private to this module. In a
/// single-module program a private tuple field is still reachable from every
/// other line of the file, and an invariant that the rest of the file can
/// bypass is a comment, not a guarantee.
mod client_pids {
    use super::Pid;

    /// A list of Zellij clients that holds at least one client.
    ///
    /// [`ClientPids::new`] is the only way to build one, so the empty case is
    /// answered once instead of at every call site. The emptiness matters
    /// because `classify_transport` asks whether *every* client of the session
    /// is a Mosh client: `all` over an empty list answers yes, so a session
    /// with no named client would be reported as Mosh for no reason. That
    /// session belongs to [`super::ZellijScan::EveryClient`] instead.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct ClientPids(Vec<Pid>);

    impl ClientPids {
        /// Wraps `pids`, or answers `None` when there is no client to wrap.
        pub(crate) fn new(pids: Vec<Pid>) -> Option<Self> {
            if pids.is_empty() {
                return None;
            }
            Some(Self(pids))
        }

        /// The clients, in the order `ps` reported them. Never empty.
        pub(crate) fn as_slice(&self) -> &[Pid] {
            &self.0
        }
    }
}

use client_pids::ClientPids;

/// Which Zellij clients stand in for the current process during the search.
///
/// Zellij daemonizes its server, so the chain from the current process stops
/// at PID 1 and never reaches a terminal. The client keeps that chain, which
/// is why the client is searched instead.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ZellijScan {
    /// Do not stand in for the current process. Used outside Zellij, and for
    /// the direct-ancestry question, which must not use the workaround.
    Off,
    /// Every process on the machine whose basename is exactly `zellij`.
    ///
    /// This is the careful answer for a session whose clients cannot be
    /// named. It can report a transport that belongs to another session.
    EveryClient,
    /// The clients that are attached to this session, found by
    /// [`zellij_client_pids`].
    ///
    /// [`ClientPids`] holds at least one client, so `classify_transport` can
    /// ask whether *every* client is a Mosh client and get an answer that a
    /// client stands behind. A session with no named client cannot arrive
    /// here at all: it uses [`ZellijScan::EveryClient`].
    Clients(ClientPids),
}

/// Determines whether a target process (identified by basename) is an ancestor
/// of the current process by analyzing parsed `ps` output.
///
/// This handles two cases:
/// 1. **Direct ancestry**: The target process is a direct ancestor of `current_pid`.
/// 2. **Zellij workaround**: Zellij daemonizes (reparents to PID 1), breaking the
///    direct ancestry. In this case, `scan` names the client processes that stand
///    in for the current process, and the target is searched above each of them.
fn find_ancestor_process(
    ps_output: &str,
    current_pid: Pid,
    scan: &ZellijScan,
    target_name: &str,
) -> bool {
    let mut parent_of: HashMap<Pid, Pid> = HashMap::new();
    let mut comm_of: HashMap<Pid, String> = HashMap::new();

    for line in ps_output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let pid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => Pid(p),
            None => continue,
        };
        let ppid = match parts.next().and_then(|s| s.parse::<u32>().ok()) {
            Some(p) => Pid(p),
            None => continue,
        };
        // Rejoin remaining tokens to reconstruct paths that contain spaces
        // (e.g., "/Users/user/my apps/mosh-server"). On macOS, `ps -eo comm=`
        // returns the full executable path. The kernel's p_comm field is limited
        // to MAXCOMLEN (16 chars), but `ps` resolves the full path via libproc,
        // so truncation is not expected. Edge cases (zombie processes, kernel
        // threads) may show truncated names; "mosh-server" (11 chars) fits.
        let comm: String = parts.collect::<Vec<&str>>().join(" ");
        parent_of.insert(pid, ppid);
        comm_of.insert(pid, comm);
    }

    // Case 1: Walk up from current PID looking for target (direct ancestry)
    let mut ancestor = current_pid;
    for _ in 0..MAX_ANCESTOR_DEPTH {
        if let Some(comm) = comm_of.get(&ancestor) {
            if comm_basename(comm) == target_name {
                return true;
            }
        }
        match parent_of.get(&ancestor) {
            Some(&ppid) if ppid != Pid(0) && ppid != ancestor => ancestor = ppid,
            _ => break,
        }
    }

    // Case 2: Inside Zellij, which daemonized and broke the ancestry chain.
    // Search above each client that stands in for the current process.
    let every_client: Vec<Pid>;
    let clients: &[Pid] = match scan {
        ZellijScan::Off => return false,
        ZellijScan::Clients(clients) => clients.as_slice(),
        // Any process whose basename is exactly "zellij" (the CLI binary, not
        // "zellij-server" or other variants).
        ZellijScan::EveryClient => {
            every_client = comm_of
                .iter()
                .filter(|(_, comm)| comm_basename(comm) == ZELLIJ_PROGRAM_NAME)
                .map(|(&pid, _)| pid)
                .collect();
            &every_client
        }
    };

    for &client in clients {
        let mut ancestor = client;
        for _ in 0..MAX_ANCESTOR_DEPTH {
            match parent_of.get(&ancestor) {
                Some(&ppid) if ppid != Pid(0) && ppid != ancestor => {
                    if let Some(pcomm) = comm_of.get(&ppid) {
                        if comm_basename(pcomm) == target_name {
                            return true;
                        }
                    }
                    ancestor = ppid;
                }
                _ => break,
            }
        }
    }

    false
}

/// Turn the older `in_zellij` flag into a scan. A session whose clients are
/// not named uses [`ZellijScan::EveryClient`], which is what `in_zellij` meant.
#[cfg(test)]
fn scan_for_flag(in_zellij: bool) -> ZellijScan {
    if in_zellij {
        ZellijScan::EveryClient
    } else {
        ZellijScan::Off
    }
}

/// Check if Mosh is in the process tree. Delegates to [`find_ancestor_process`].
#[cfg(test)]
fn has_mosh_in_process_tree(ps_output: &str, current_pid: Pid, in_zellij: bool) -> bool {
    find_ancestor_process(
        ps_output,
        current_pid,
        &scan_for_flag(in_zellij),
        "mosh-server",
    )
}

/// Check if Eternal Terminal is in the process tree. Delegates to [`find_ancestor_process`].
/// Looks for `etterminal` (the per-session worker), not `etserver` (the daemon),
/// because `etterminal` is the direct ancestor of the user's shell.
#[cfg(test)]
fn has_et_in_process_tree(ps_output: &str, current_pid: Pid, in_zellij: bool) -> bool {
    find_ancestor_process(
        ps_output,
        current_pid,
        &scan_for_flag(in_zellij),
        "etterminal",
    )
}

/// Write an image with the Kitty graphics protocol.
///
/// The command is `ESC _ G <key>=<value>,... ; <base64 data> ESC \`. A large
/// image goes out in more than one command, because the protocol limits the
/// size of one command.
///
/// The routine holds the cursor still with `C=1` and then states the position
/// of the cursor itself through [`write_image_with_cursor_contract`].
///
/// # Arguments
/// * `rgb_data` - The pixels of the image, three bytes for each pixel.
/// * `img_width` - The width of the image in pixels.
/// * `img_height` - The height of the image in pixels.
/// * `display_width` - The width that `ic` asks for, in character cells.
/// * `display_height` - The height that `ic` asks for, in character cells.
/// * `no_newline` - True when the caller controls the position of the cursor.
///
/// # Returns
/// An error when a write to stdout fails.
fn print_kitty_image(
    rgb_data: &[u8],
    img_width: u32,
    img_height: u32,
    display_width: Option<u32>,
    display_height: Option<u32>,
    no_newline: bool,
) -> Result<()> {
    let mut stdout = io::stdout().lock();

    let base64_data = BASE64_STANDARD.encode(rgb_data);
    let chunk_size = 4096; // Kitty recommended chunk size

    let contract = if no_newline {
        CursorContract::CallerManaged
    } else {
        let (cell_width_px, cell_height_px) = get_cell_pixel_dimensions();
        CursorContract::BelowImage {
            rows: image_rows_in_cells(
                img_width,
                img_height,
                display_width,
                display_height,
                cell_width_px,
                cell_height_px,
            ),
        }
    };

    // The key list that opens the first command. `C=1` tells Kitty not to move
    // the cursor, because this routine states the position of the cursor
    // itself. In video mode, fixed image and placement ids make each frame
    // replace the last one in place, which holds the memory of the renderer
    // flat.
    let cursor_keys = if no_newline { ",i=1,p=1,C=1" } else { ",C=1" };
    // The display size, in character cells.
    let width_key = display_width.map_or_else(String::new, |w| format!(",c={w}"));
    let height_key = display_height.map_or_else(String::new, |h| format!(",r={h}"));
    let header =
        format!("\x1b_Ga=T,f=24,s={img_width},v={img_height}{cursor_keys}{width_key}{height_key}");

    write_image_with_cursor_contract(&mut stdout, contract, |out| {
        if base64_data.len() <= chunk_size {
            // A small image goes out in one command.
            return write!(out, "{header};{base64_data}\x1b\\");
        }

        // A large image goes out in more than one command. `m=1` says that more
        // data follows and `m=0` closes the image.
        let chunks: Vec<&str> = base64_data
            .as_bytes()
            .chunks(chunk_size)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect();

        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                write!(out, "{header},m=1;{chunk}\x1b\\")?;
            } else if i == chunks.len() - 1 {
                write!(out, "\x1b_Gm=0;{chunk}\x1b\\")?;
            } else {
                write!(out, "\x1b_Gm=1;{chunk}\x1b\\")?;
            }
        }

        Ok(())
    })?;

    stdout.flush().context("Failed to flush output")?;
    Ok(())
}

/// Write an image with the iTerm2 inline image protocol.
///
/// The command is `ESC ] 1337 ; File = <arguments> : <base64 data> BEL`.
///
/// The routine holds the cursor still with `doNotMoveCursor=1` and then states
/// the position of the cursor itself through
/// [`write_image_with_cursor_contract`].
///
/// # Arguments
/// * `base64_data` - The image, in base64.
/// * `img_width_px` - The width of the image in pixels.
/// * `img_height_px` - The height of the image in pixels.
/// * `width` - The width that `ic` asks for, in character cells.
/// * `height` - The height that `ic` asks for, in character cells.
/// * `no_newline` - True when the caller controls the position of the cursor.
///
/// # Returns
/// An error when a write to stdout fails.
fn print_iterm2_image(
    base64_data: &str,
    img_width_px: u32,
    img_height_px: u32,
    width: Option<u32>,
    height: Option<u32>,
    no_newline: bool,
) -> Result<()> {
    let mut stdout = io::stdout().lock();

    let contract = if no_newline {
        CursorContract::CallerManaged
    } else {
        let (cell_width_px, cell_height_px) = get_cell_pixel_dimensions();
        CursorContract::BelowImage {
            rows: image_rows_in_cells(
                img_width_px,
                img_height_px,
                width,
                height,
                cell_width_px,
                cell_height_px,
            ),
        }
    };

    // The width and the height are in character cells, so they carry no `px`
    // suffix. `doNotMoveCursor=1` tells iTerm2 not to move the cursor, because
    // this routine states the position of the cursor itself.
    let width_argument = width.map_or_else(String::new, |w| format!(";width={w}"));
    let height_argument = height.map_or_else(String::new, |h| format!(";height={h}"));

    write_image_with_cursor_contract(&mut stdout, contract, |out| {
        write!(
            out,
            "\x1b]1337;File=inline=1{width_argument}{height_argument};preserveAspectRatio=1;doNotMoveCursor=1:{base64_data}\x07"
        )
    })?;

    stdout.flush().context("Failed to flush output")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Tests for calculate_aspect_preserving_size
    // =========================================================================

    /// Standard cell aspect ratio used in tests (typical terminal: cells ~2x tall as wide).
    const TEST_CELL_ASPECT: f64 = 2.0;

    #[test]
    fn aspect_preserving_returns_original_when_disabled() {
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), Some(50), false, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), Some(50)));
    }

    #[test]
    fn aspect_preserving_handles_zero_height_defensively() {
        // Zero height should not panic (division by zero), returns original dimensions
        let result =
            calculate_aspect_preserving_size(100, 0, Some(50), Some(50), true, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), Some(50)));
    }

    #[test]
    fn aspect_preserving_square_image_in_square_box() {
        // Square image (100x100) in square box (50x50)
        // With cell aspect ratio of 2:1, effective box is 50x100 pixels
        // Image aspect = 1.0, box aspect = 50/100 = 0.5
        // Image is wider than box, constrain by width
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_width = 50, display_height = (50/1.0)/2.0 = 25
        assert_eq!(result, (Some(50), Some(25)));
    }

    #[test]
    fn aspect_preserving_wide_image() {
        // Wide image (200x100) in square box (50x50)
        // Image aspect = 2.0
        // Constrain by width
        let result =
            calculate_aspect_preserving_size(200, 100, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_width = 50, display_height = (50/2.0)/2.0 = 12.5 -> 13 (rounded)
        assert_eq!(result, (Some(50), Some(13)));
    }

    #[test]
    fn aspect_preserving_tall_image() {
        // Tall image (100x400) in square box (50x50)
        // Image aspect = 0.25
        // Constrain by height
        let result =
            calculate_aspect_preserving_size(100, 400, Some(50), Some(50), true, TEST_CELL_ASPECT);
        // display_height = 50, display_width = 50 * 2.0 * 0.25 = 25
        assert_eq!(result, (Some(25), Some(50)));
    }

    #[test]
    fn aspect_preserving_only_width_specified() {
        let result =
            calculate_aspect_preserving_size(100, 100, Some(50), None, true, TEST_CELL_ASPECT);
        assert_eq!(result, (Some(50), None));
    }

    #[test]
    fn aspect_preserving_only_height_specified() {
        let result =
            calculate_aspect_preserving_size(100, 100, None, Some(50), true, TEST_CELL_ASPECT);
        assert_eq!(result, (None, Some(50)));
    }

    #[test]
    fn aspect_preserving_no_dimensions_specified() {
        let result = calculate_aspect_preserving_size(100, 100, None, None, true, TEST_CELL_ASPECT);
        assert_eq!(result, (None, None));
    }

    #[test]
    fn aspect_preserving_minimum_dimension_is_one() {
        // Very wide image that would result in height < 1
        let result =
            calculate_aspect_preserving_size(10000, 1, Some(10), Some(10), true, TEST_CELL_ASPECT);
        // Should clamp height to at least 1
        assert!(result.1.unwrap() >= 1);

        // Very tall image that would result in width < 1
        let result =
            calculate_aspect_preserving_size(1, 10000, Some(10), Some(10), true, TEST_CELL_ASPECT);
        // Should clamp width to at least 1
        assert!(result.0.unwrap() >= 1);
    }

    // =========================================================================
    // Tests for calculate_sixel_dimensions
    // =========================================================================

    #[test]
    fn sixel_returns_target_when_aspect_disabled() {
        let result = calculate_sixel_dimensions(100, 100, 800, 600, false);
        assert_eq!(result, (800, 600));
    }

    #[test]
    fn sixel_square_image_in_landscape_target() {
        // Square image (100x100) in landscape target (800x600)
        // Image aspect = 1.0, target aspect = 800/600 = 1.33
        // Image is taller than target, constrain by height
        let result = calculate_sixel_dimensions(100, 100, 800, 600, true);
        // h = 600, w = 600 * 1.0 = 600
        assert_eq!(result, (600, 600));
    }

    #[test]
    fn sixel_wide_image_in_landscape_target() {
        // Wide image (1600x900, 16:9) in landscape target (800x600)
        // Image aspect = 1.78, target aspect = 1.33
        // Image is wider, constrain by width
        let result = calculate_sixel_dimensions(1600, 900, 800, 600, true);
        // w = 800, h = 800 / 1.78 = 450
        assert_eq!(result, (800, 450));
    }

    #[test]
    fn sixel_tall_image_in_landscape_target() {
        // Tall image (100x400) in landscape target (800x600)
        // Image aspect = 0.25, target aspect = 1.33
        // Image is taller, constrain by height
        let result = calculate_sixel_dimensions(100, 400, 800, 600, true);
        // h = 600, w = 600 * 0.25 = 150
        assert_eq!(result, (150, 600));
    }

    #[test]
    fn sixel_minimum_dimension_is_one() {
        // Very wide image
        let result = calculate_sixel_dimensions(10000, 1, 100, 100, true);
        assert!(result.1 >= 1);

        // Very tall image
        let result = calculate_sixel_dimensions(1, 10000, 100, 100, true);
        assert!(result.0 >= 1);
    }

    #[test]
    fn sixel_handles_zero_height_defensively() {
        // Zero height should produce infinity aspect, which comparing > target_aspect
        // will be true, so we'll constrain by width
        // w = 100, h = 100 / infinity = 0, clamped to 1
        let result = calculate_sixel_dimensions(100, 0, 100, 100, true);
        assert_eq!(result.1, 1); // Height should be clamped to 1
    }

    // =========================================================================
    // Tests for terminal type detection (basic sanity checks)
    // =========================================================================

    #[test]
    fn terminal_type_enum_is_exhaustive() {
        // Ensure we can construct all terminal types (compile-time check)
        let _kitty = TerminalType::Kitty;
        let _ghostty = TerminalType::Ghostty;
        let _iterm2 = TerminalType::ITerm2;
        let _wezterm = TerminalType::WezTerm;
        let _alacritty = TerminalType::Alacritty;
        let _zellij = TerminalType::Zellij;
        let _muxiavelli = TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel);
        let _unknown = TerminalType::Unknown;
    }

    // =========================================================================
    // Tests for muxiavelli image-protocol selection (select_muxiavelli_protocol)
    // =========================================================================

    #[test]
    fn muxiavelli_protocol_absent_falls_back_to_sixel() {
        assert_eq!(
            select_muxiavelli_protocol(None),
            MuxiavelliImageProtocol::Sixel
        );
    }

    #[test]
    fn muxiavelli_protocol_empty_falls_back_to_sixel() {
        assert_eq!(
            select_muxiavelli_protocol(Some("")),
            MuxiavelliImageProtocol::Sixel
        );
    }

    #[test]
    fn muxiavelli_protocol_unrecognized_falls_back_to_sixel() {
        // Kitty (and any other token `ic` cannot emit) must never be selected.
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty")),
            MuxiavelliImageProtocol::Sixel
        );
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty,png,webp")),
            MuxiavelliImageProtocol::Sixel
        );
    }

    #[test]
    fn muxiavelli_protocol_honors_advertised_order() {
        // Proves the list is honored in order, not hardcoded to Sixel.
        assert_eq!(
            select_muxiavelli_protocol(Some("sixel,iterm2")),
            MuxiavelliImageProtocol::Sixel
        );
        assert_eq!(
            select_muxiavelli_protocol(Some("iterm2,sixel")),
            MuxiavelliImageProtocol::Iterm2
        );
    }

    #[test]
    fn muxiavelli_protocol_skips_unsupported_then_picks_supported() {
        // An unsupported leading token is skipped, not treated as a fallback.
        assert_eq!(
            select_muxiavelli_protocol(Some("kitty,iterm2")),
            MuxiavelliImageProtocol::Iterm2
        );
    }

    #[test]
    fn muxiavelli_protocol_tolerates_whitespace_and_case() {
        assert_eq!(
            select_muxiavelli_protocol(Some("  ITERM2 , sixel ")),
            MuxiavelliImageProtocol::Iterm2
        );
    }

    // =========================================================================
    // Tests for classify_terminal_type (precedence + existing detection)
    // =========================================================================

    /// A muxiavelli panel whose PTY leaked the host terminal's env vars: Kitty
    /// **and** Ghostty signals are present simultaneously. MUXIAVELLI must still
    /// win over both. This is the exact mis-detection the issue describes.
    fn leaked_muxiavelli_env(protocols: Option<&str>) -> TerminalEnv {
        TerminalEnv {
            term: "xterm-ghostty".to_string(),
            term_program: "ghostty".to_string(),
            zellij: false,
            muxiavelli: true,
            muxiavelli_protocols: protocols.map(str::to_string),
            kitty_window_id: true,
            ghostty_resources_dir: true,
            iterm_session_id: false,
            alacritty_socket: false,
        }
    }

    #[test]
    fn muxiavelli_wins_over_leaked_host_signals_and_selects_sixel() {
        let env = leaked_muxiavelli_env(Some("sixel,iterm2"));
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel)
        );
    }

    #[test]
    fn muxiavelli_iterm2_first_selects_iterm2_despite_leak() {
        let env = leaked_muxiavelli_env(Some("iterm2,sixel"));
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(MuxiavelliImageProtocol::Iterm2)
        );
    }

    #[test]
    fn muxiavelli_absent_protocols_defaults_to_sixel() {
        let env = leaked_muxiavelli_env(None);
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel)
        );
    }

    #[test]
    fn muxiavelli_wins_over_zellij_backend() {
        // muxiavelli may run a zellij backing session, so ZELLIJ can also be set
        // inside a panel; MUXIAVELLI must still win since it is the precise signal.
        let mut env = leaked_muxiavelli_env(None);
        env.zellij = true;
        assert_eq!(
            classify_terminal_type(&env),
            TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel)
        );
    }

    #[test]
    fn classify_zellij_when_not_muxiavelli() {
        let env = TerminalEnv {
            zellij: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Zellij);
    }

    #[test]
    fn classify_kitty_from_window_id() {
        let env = TerminalEnv {
            kitty_window_id: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Kitty);
    }

    #[test]
    fn classify_kitty_from_term() {
        let env = TerminalEnv {
            term: "xterm-kitty".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Kitty);
    }

    #[test]
    fn classify_ghostty_from_term_program() {
        let env = TerminalEnv {
            term_program: "ghostty".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Ghostty);
    }

    #[test]
    fn classify_iterm2_from_session_id() {
        let env = TerminalEnv {
            iterm_session_id: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::ITerm2);
    }

    #[test]
    fn classify_iterm2_from_term_program() {
        let env = TerminalEnv {
            term_program: "iTerm.app".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::ITerm2);
    }

    #[test]
    fn classify_wezterm_from_term_program() {
        let env = TerminalEnv {
            term_program: "WezTerm".to_string(),
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::WezTerm);
    }

    #[test]
    fn classify_alacritty_from_socket() {
        let env = TerminalEnv {
            alacritty_socket: true,
            ..TerminalEnv::default()
        };
        assert_eq!(classify_terminal_type(&env), TerminalType::Alacritty);
    }

    #[test]
    fn classify_unknown_when_no_signals() {
        assert_eq!(
            classify_terminal_type(&TerminalEnv::default()),
            TerminalType::Unknown
        );
    }

    // =========================================================================
    // Tests for terminal_supports_graphics
    // =========================================================================

    #[test]
    fn muxiavelli_supports_graphics_regardless_of_term() {
        // Even if a backing session leaks TERM=screen/vt, muxiavelli renders images.
        assert!(terminal_supports_graphics(
            &TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel),
            "screen.xterm-256color"
        ));
        assert!(terminal_supports_graphics(
            &TerminalType::Muxiavelli(MuxiavelliImageProtocol::Iterm2),
            "vt100"
        ));
    }

    #[test]
    fn alacritty_never_supports_graphics() {
        assert!(!terminal_supports_graphics(
            &TerminalType::Alacritty,
            "xterm-256color"
        ));
    }

    #[test]
    fn kitty_supports_graphics_on_normal_term() {
        assert!(terminal_supports_graphics(
            &TerminalType::Kitty,
            "xterm-kitty"
        ));
    }

    #[test]
    fn linux_console_does_not_support_graphics() {
        assert!(!terminal_supports_graphics(&TerminalType::Unknown, "linux"));
    }

    // =========================================================================
    // Tests for display_routine_for (dispatch routing)
    // =========================================================================

    #[test]
    fn dispatch_muxiavelli_sixel_routes_to_sixel() {
        assert_eq!(
            display_routine_for(&TerminalType::Muxiavelli(MuxiavelliImageProtocol::Sixel)),
            DisplayRoutine::Sixel
        );
    }

    #[test]
    fn dispatch_muxiavelli_iterm2_routes_to_iterm2() {
        assert_eq!(
            display_routine_for(&TerminalType::Muxiavelli(MuxiavelliImageProtocol::Iterm2)),
            DisplayRoutine::Iterm2
        );
    }

    #[test]
    fn dispatch_zellij_routes_to_sixel() {
        assert_eq!(
            display_routine_for(&TerminalType::Zellij),
            DisplayRoutine::Sixel
        );
    }

    #[test]
    fn dispatch_kitty_family_routes_to_kitty() {
        for terminal_type in [
            TerminalType::Kitty,
            TerminalType::Ghostty,
            TerminalType::WezTerm,
        ] {
            assert_eq!(display_routine_for(&terminal_type), DisplayRoutine::Kitty);
        }
    }

    #[test]
    fn dispatch_other_terminals_route_to_iterm2() {
        for terminal_type in [
            TerminalType::ITerm2,
            TerminalType::Alacritty,
            TerminalType::Unknown,
        ] {
            assert_eq!(display_routine_for(&terminal_type), DisplayRoutine::Iterm2);
        }
    }

    // =========================================================================
    // Tests verifying constants are reasonable
    // =========================================================================

    #[test]
    fn constants_have_reasonable_values() {
        // Derived cell aspect ratio (from fallback constants) should be reasonable (1.5 to 3.0)
        let fallback_aspect = ESTIMATED_CELL_HEIGHT_PX as f64 / ESTIMATED_CELL_WIDTH_PX as f64;
        assert!(fallback_aspect > 1.0);
        assert!(fallback_aspect < 4.0);

        // Cell pixel estimates should be positive and reasonable
        const { assert!(ESTIMATED_CELL_WIDTH_PX >= 6) };
        const { assert!(ESTIMATED_CELL_WIDTH_PX <= 20) };
        const { assert!(ESTIMATED_CELL_HEIGHT_PX >= 12) };
        const { assert!(ESTIMATED_CELL_HEIGHT_PX <= 40) };

        // Default sixel dimensions should be reasonable screen sizes
        const { assert!(DEFAULT_SIXEL_WIDTH_PX >= 640) };
        const { assert!(DEFAULT_SIXEL_HEIGHT_PX >= 480) };

        // Margins should be between 0.5 and 1.0
        const { assert!(SIXEL_HORIZONTAL_MARGIN > 0.5) };
        const { assert!(SIXEL_HORIZONTAL_MARGIN <= 1.0) };
        const { assert!(SIXEL_VERTICAL_MARGIN > 0.5) };
        const { assert!(SIXEL_VERTICAL_MARGIN <= 1.0) };
    }

    // =========================================================================
    // Tests for comm_basename
    // =========================================================================

    #[test]
    fn comm_basename_full_path() {
        assert_eq!(comm_basename("/usr/local/bin/mosh-server"), "mosh-server");
    }

    #[test]
    fn comm_basename_bare_name() {
        assert_eq!(comm_basename("mosh-server"), "mosh-server");
    }

    #[test]
    fn comm_basename_path_with_spaces() {
        // Paths with spaces are reconstructed by the join(" ") in parsing
        assert_eq!(
            comm_basename("/Users/user/my apps/mosh-server"),
            "mosh-server"
        );
    }

    #[test]
    fn comm_basename_empty_string() {
        assert_eq!(comm_basename(""), "");
    }

    // =========================================================================
    // Tests for has_mosh_in_process_tree
    // =========================================================================

    #[test]
    fn mosh_detected_bare_session() {
        // Process tree: mosh-server(100) -> bash(200) -> ic(300)
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(has_mosh_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn mosh_detected_bare_name() {
        // comm is just the bare name, no path
        let ps_output = "\
  100     1 mosh-server
  200   100 bash
  300   200 ic";
        assert!(has_mosh_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn mosh_detected_in_zellij() {
        // mosh-server(100) -> zellij CLI(200), but current process(400)
        // is child of zellij-server(300) which reparented to PID 1
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid(400), true));
    }

    #[test]
    fn no_mosh_in_normal_session() {
        let ps_output = "\
    1     0 /sbin/launchd
  500     1 /usr/sbin/sshd
  600   500 /bin/bash
  700   600 /usr/local/bin/ic";
        assert!(!has_mosh_in_process_tree(ps_output, Pid(700), false));
    }

    #[test]
    fn mosh_empty_ps_output() {
        assert!(!has_mosh_in_process_tree("", Pid(1), false));
    }

    #[test]
    fn mosh_malformed_lines_skipped() {
        let ps_output = "\
not_a_number  1 /bin/bash
  100     1 /usr/bin/mosh-server
  abc   def /foo/bar
  200   100 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid(200), false));
    }

    #[test]
    fn mosh_zellij_exact_match_no_false_positive() {
        // "my-zellij-wrapper" and "zellij-server" must NOT match as zellij CLI
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/local/bin/my-zellij-wrapper
  300     1 /usr/local/bin/zellij-server
  400   300 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid(400), true));
    }

    #[test]
    fn mosh_server_exact_match_no_false_positive() {
        // "mosh-server-wrapper" must NOT match as mosh-server
        let ps_output = "\
  100     1 /usr/bin/mosh-server-wrapper
  200   100 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid(200), false));
    }

    #[test]
    fn mosh_comm_path_with_spaces() {
        // Paths with spaces are reconstructed by join(" "),
        // and comm_basename extracts the correct filename
        let ps_output = "\
  100     1 /Users/user/my apps/mosh-server
  200   100 /bin/bash";
        assert!(has_mosh_in_process_tree(ps_output, Pid(200), false));
    }

    #[test]
    fn mosh_current_pid_not_in_table() {
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash";
        // PID 999 is not in the table
        assert!(!has_mosh_in_process_tree(ps_output, Pid(999), false));
    }

    #[test]
    fn mosh_cycle_does_not_loop_forever() {
        // A process whose parent is itself should not cause infinite loop
        let ps_output = "\
  100   100 /bin/bash";
        assert!(!has_mosh_in_process_tree(ps_output, Pid(100), false));
    }

    #[test]
    fn mosh_not_detected_when_zellij_env_unset() {
        // Even though a zellij process exists under mosh-server,
        // Case 2 should not trigger when in_zellij is false
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        // current PID 400 is not an ancestor of mosh-server via Case 1,
        // and in_zellij=false disables Case 2
        assert!(!has_mosh_in_process_tree(ps_output, Pid(400), false));
    }

    // =========================================================================
    // Tests for the Mosh message
    // =========================================================================

    /// Ask for the Mosh message that `ic` prints when it refuses an image.
    fn mosh_refusal_message() -> String {
        let caps = TerminalCapabilities {
            terminal_type: TerminalType::ITerm2,
            supports_graphics: true,
            supports_raw_mode: true,
        };
        let error = validate_terminal_for_graphics(&caps, &RemoteTransport::Mosh, "Image")
            .expect_err("Mosh must be refused");
        error.to_string()
    }

    #[test]
    fn the_mosh_message_does_not_recommend_eternal_terminal() {
        // Eternal Terminal drops the session when the laptop sleeps, which is
        // the reconnect that matters here. Do not send the reader to it.
        let message = mosh_refusal_message();
        assert!(
            !message.contains("Eternal Terminal"),
            "message still recommends Eternal Terminal: {message}"
        );
        assert!(
            !message.contains("et user@host"),
            "message still recommends the et command: {message}"
        );
    }

    #[test]
    fn the_mosh_message_still_offers_ssh() {
        assert!(mosh_refusal_message().contains("ssh user@host"));
    }

    // =========================================================================
    // Tests for classify_transport (the whole transport decision)
    // =========================================================================

    /// The PID of the process that asks for the transport. It sits under the
    /// Zellij server of the session named `ic-test`.
    const CURRENT: Pid = Pid(63481);

    /// A process table that holds two Zellij sessions.
    ///
    /// `ic-test` is viewed over SSH. `meshtastic` is viewed over Mosh. The
    /// Zellij server of each session is reparented to PID 1, which is why the
    /// chain from `CURRENT` reaches no terminal.
    ///
    /// The basename of the server is `zellij`, not `zellij-server`. That is
    /// what macOS reports, because the server is the same binary started with
    /// `--server`.
    const PS_COMM_TWO_SESSIONS: &str = "\
    1     0 /sbin/launchd
56659     1 sshd-session
56665 56659 sshd-session
56666 56665 -zsh
57053 56666 /Users/t/.local/bin/zellij
51648     1 /Users/t/.local/bin/zellij
63481 51648 /bin/zsh
31465     1 /usr/bin/mosh-server
31466 31465 -zsh
32269 31466 /Users/t/.local/bin/zellij";

    const PS_ARGS_TWO_SESSIONS_FULL: &str = "\
51648 /Users/t/.local/bin/zellij --server /tmp/zellij-501/contract_version_1/ic-test
57053 zellij a ic-test
32269 zellij a meshtastic";

    #[test]
    fn another_sessions_mosh_client_does_not_decide_this_session() {
        // The client of `ic-test` runs under SSH. The Mosh client belongs to
        // `meshtastic`, so it says nothing about this session.
        assert_eq!(
            classify_transport(
                PS_COMM_TWO_SESSIONS,
                PS_ARGS_TWO_SESSIONS_FULL,
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::None
        );
    }

    #[test]
    fn mosh_detected_when_the_only_client_of_this_session_is_mosh() {
        let ps_comm = "\
    1     0 /sbin/launchd
51648     1 /Users/t/.local/bin/zellij
63481 51648 /bin/zsh
31465     1 /usr/bin/mosh-server
31466 31465 -zsh
57100 31466 /Users/t/.local/bin/zellij";
        let ps_args = "\
51648 /Users/t/.local/bin/zellij --server /tmp/zellij-501/contract_version_1/ic-test
57100 zellij a ic-test";
        assert_eq!(
            classify_transport(
                ps_comm,
                ps_args,
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::Mosh
        );
    }

    #[test]
    fn one_capable_client_wins_over_a_mosh_client_of_the_same_session() {
        // Zellij sends the output to every attached client. An SSH viewer can
        // show the image. The Mosh viewer ignores the escape sequences.
        let ps_comm = "\
    1     0 /sbin/launchd
51648     1 /Users/t/.local/bin/zellij
63481 51648 /bin/zsh
56665     1 sshd-session
56666 56665 -zsh
57053 56666 /Users/t/.local/bin/zellij
31465     1 /usr/bin/mosh-server
31466 31465 -zsh
57100 31466 /Users/t/.local/bin/zellij";
        let ps_args = "\
57053 zellij a ic-test
57100 zellij a ic-test";
        assert_eq!(
            classify_transport(
                ps_comm,
                ps_args,
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::None
        );
    }

    #[test]
    fn an_unnamed_zellij_session_keeps_the_careful_answer() {
        // ZELLIJ_SESSION_NAME is unset, so no client can be tied to this
        // session. Report Mosh rather than send escape sequences that Mosh
        // strips.
        assert_eq!(
            classify_transport(
                PS_COMM_TWO_SESSIONS,
                PS_ARGS_TWO_SESSIONS_FULL,
                CURRENT,
                Some(String::new()),
                false,
            ),
            RemoteTransport::Mosh
        );
    }

    #[test]
    fn a_session_with_no_named_client_keeps_the_careful_answer() {
        // The session name is known, but no client argv carries it. This
        // happens when Zellij starts with a generated session name.
        assert_eq!(
            classify_transport(
                PS_COMM_TWO_SESSIONS,
                "",
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::Mosh
        );
    }

    #[test]
    fn outside_zellij_a_mosh_client_of_a_session_is_ignored() {
        assert_eq!(
            classify_transport(
                PS_COMM_TWO_SESSIONS,
                PS_ARGS_TWO_SESSIONS_FULL,
                CURRENT,
                None,
                false,
            ),
            RemoteTransport::None
        );
    }

    #[test]
    fn direct_mosh_ancestry_beats_every_client_rule() {
        let ps_comm = "\
31465     1 /usr/bin/mosh-server
31466 31465 -zsh
63481 31466 /bin/zsh";
        assert_eq!(
            classify_transport(
                ps_comm,
                "57053 zellij a ic-test",
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::Mosh
        );
    }

    #[test]
    fn eternal_terminal_env_var_beats_a_mosh_client() {
        assert_eq!(
            classify_transport(
                PS_COMM_TWO_SESSIONS,
                PS_ARGS_TWO_SESSIONS_FULL,
                CURRENT,
                Some("ic-test".to_string()),
                true,
            ),
            RemoteTransport::EternalTerminal
        );
    }

    #[test]
    fn an_eternal_terminal_client_of_this_session_wins() {
        let ps_comm = "\
    1     0 /sbin/launchd
51648     1 /Users/t/.local/bin/zellij
63481 51648 /bin/zsh
40000     1 /usr/local/bin/etterminal
40001 40000 -zsh
57053 40001 /Users/t/.local/bin/zellij";
        assert_eq!(
            classify_transport(
                ps_comm,
                "57053 zellij a ic-test",
                CURRENT,
                Some("ic-test".to_string()),
                false,
            ),
            RemoteTransport::EternalTerminal
        );
    }

    // =========================================================================
    // Tests for zellij_client_pids (session-scoped client discovery)
    // =========================================================================

    /// A `ps -eo pid=,args=` table with two Zellij sessions and one server.
    const PS_ARGS_TWO_SESSIONS: &str = "\
  51648 /Users/t/.local/bin/zellij --server /tmp/zellij-501/contract_version_1/ic-test
  57053 zellij a ic-test
  32269 zellij a meshtastic
  56666 -zsh";

    #[test]
    fn zellij_client_pids_finds_the_client_of_this_session() {
        assert_eq!(
            zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test"),
            vec![Pid(57053)]
        );
    }

    #[test]
    fn zellij_client_pids_ignores_another_sessions_client() {
        let found = zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test");
        assert!(!found.contains(&Pid(32269)));
    }

    #[test]
    fn zellij_client_pids_ignores_the_server_of_this_session() {
        // The server has the session name in its socket path, not as an
        // argument of its own. It is not a client.
        let found = zellij_client_pids(PS_ARGS_TWO_SESSIONS, "ic-test");
        assert!(!found.contains(&Pid(51648)));
    }

    #[test]
    fn zellij_client_pids_accepts_every_attach_form() {
        let ps_args = "\
  100 zellij a work
  200 zellij attach work
  300 zellij -s work
  400 zellij --session work";
        assert_eq!(
            zellij_client_pids(ps_args, "work"),
            vec![Pid(100), Pid(200), Pid(300), Pid(400)]
        );
    }

    #[test]
    fn zellij_client_pids_requires_a_whole_argument_match() {
        // "work" must not match the session named "work-tree".
        let ps_args = "  100 zellij a work-tree";
        assert!(zellij_client_pids(ps_args, "work").is_empty());
    }

    #[test]
    fn zellij_client_pids_requires_an_exact_program_name() {
        // A wrapper script named "my-zellij-wrapper" is not the Zellij CLI.
        let ps_args = "\
  100 /usr/local/bin/my-zellij-wrapper a work
  200 /usr/local/bin/zellij-server a work";
        assert!(zellij_client_pids(ps_args, "work").is_empty());
    }

    #[test]
    fn zellij_client_pids_is_empty_for_an_unknown_session() {
        assert!(zellij_client_pids(PS_ARGS_TWO_SESSIONS, "no-such-session").is_empty());
    }

    #[test]
    fn zellij_client_pids_handles_empty_input() {
        assert!(zellij_client_pids("", "ic-test").is_empty());
    }

    #[test]
    fn zellij_client_pids_skips_malformed_lines() {
        let ps_args = "\
not_a_number zellij a work
  100 zellij a work";
        assert_eq!(zellij_client_pids(ps_args, "work"), vec![Pid(100)]);
    }

    #[test]
    fn zellij_client_pids_ignores_an_empty_session_name() {
        // ZELLIJ_SESSION_NAME is unset or empty. No client can be identified.
        assert!(zellij_client_pids(PS_ARGS_TWO_SESSIONS, "").is_empty());
    }

    // =========================================================================
    // Tests for ClientPids (the client list that cannot be empty)
    // =========================================================================

    #[test]
    fn client_pids_refuses_an_empty_list() {
        // An empty list would answer "every client is a Mosh client" for no
        // reason, so it must not be possible to build one.
        assert!(ClientPids::new(Vec::new()).is_none());
    }

    #[test]
    fn client_pids_keeps_a_non_empty_list_in_order() {
        let clients =
            ClientPids::new(vec![Pid(57053), Pid(32269)]).expect("a list with clients is accepted");
        assert_eq!(clients.as_slice(), [Pid(57053), Pid(32269)]);
    }

    // =========================================================================
    // Tests for has_et_in_process_tree (Eternal Terminal detection)
    // =========================================================================

    #[test]
    fn et_detected_bare_session() {
        // Process tree: etterminal(100) -> bash(200) -> ic(300)
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(has_et_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn et_detected_bare_name() {
        // comm is just the bare name, no path
        let ps_output = "\
  100     1 etterminal
  200   100 bash
  300   200 ic";
        assert!(has_et_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn et_detected_in_zellij() {
        // etterminal(100) -> zellij CLI(200), but current process(400)
        // is child of zellij-server(300) which reparented to PID 1
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /usr/bin/zellij
  300     1 /usr/bin/zellij-server
  400   300 /bin/bash";
        assert!(has_et_in_process_tree(ps_output, Pid(400), true));
    }

    #[test]
    fn no_et_in_normal_session() {
        let ps_output = "\
    1     0 /sbin/launchd
  500     1 /usr/sbin/sshd
  600   500 /bin/bash
  700   600 /usr/local/bin/ic";
        assert!(!has_et_in_process_tree(ps_output, Pid(700), false));
    }

    #[test]
    fn et_not_confused_with_mosh() {
        // mosh-server present but no etterminal
        let ps_output = "\
  100     1 /usr/bin/mosh-server
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(!has_et_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn mosh_not_confused_with_et() {
        // etterminal present but no mosh-server
        let ps_output = "\
  100     1 /usr/bin/etterminal
  200   100 /bin/bash
  300   200 /usr/local/bin/ic";
        assert!(!has_mosh_in_process_tree(ps_output, Pid(300), false));
    }

    #[test]
    fn et_exact_match_no_false_positive() {
        // "etterminal-wrapper" must NOT match as etterminal
        let ps_output = "\
  100     1 /usr/bin/etterminal-wrapper
  200   100 /bin/bash";
        assert!(!has_et_in_process_tree(ps_output, Pid(200), false));
    }

    // =========================================================================
    // Tests for cells_to_pixels
    // =========================================================================

    #[test]
    fn cells_to_pixels_basic() {
        assert_eq!(cells_to_pixels(80, 24, 10, 20), (800, 480));
    }

    #[test]
    fn cells_to_pixels_single_cell() {
        assert_eq!(cells_to_pixels(1, 1, 10, 20), (10, 20));
    }

    #[test]
    fn cells_to_pixels_custom_cell_dimensions() {
        assert_eq!(cells_to_pixels(40, 12, 16, 32), (640, 384));
    }

    // =========================================================================
    // Tests for downscale_to_display_pixels
    // =========================================================================

    /// Helper to create a test image of the given dimensions.
    fn make_test_image(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
    }

    #[test]
    fn downscale_returns_borrowed_when_no_dimensions() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, None, None);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_width() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, Some(50), None);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_only_height() {
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, None, Some(50));
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn downscale_returns_borrowed_when_image_fits() {
        // With estimated cell dimensions (10x20), 80 cols x 24 rows = 800x480 pixels.
        // A 100x100 image fits within that, so no downscale needed.
        // Note: in CI/test environments, get_terminal_pixel_size() returns None,
        // so estimated constants are used.
        let img = make_test_image(100, 100);
        let result = downscale_to_display_pixels(&img, Some(80), Some(24));
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.width(), 100);
        assert_eq!(result.height(), 100);
    }

    #[test]
    fn downscale_shrinks_oversized_image() {
        // 10 cols x 5 rows, pixel target depends on actual cell dimensions
        let (cell_w, cell_h) = get_cell_pixel_dimensions();
        let (max_w, max_h) = cells_to_pixels(10, 5, cell_w, cell_h);
        // Image must be larger than the target to trigger downscaling
        let img = make_test_image(max_w * 5, max_h * 5);
        let result = downscale_to_display_pixels(&img, Some(10), Some(5));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
    }

    #[test]
    fn downscale_preserves_aspect_ratio() {
        // Wide image with 5:1 aspect ratio, larger than any reasonable target
        let (cell_w, cell_h) = get_cell_pixel_dimensions();
        let (max_w, max_h) = cells_to_pixels(10, 5, cell_w, cell_h);
        let img = make_test_image(max_w * 10, max_h * 2);
        let result = downscale_to_display_pixels(&img, Some(10), Some(5));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
        // Aspect ratio should be roughly maintained (5:1)
        let aspect = result.width() as f64 / result.height() as f64;
        assert!(
            (aspect - 5.0).abs() < 0.5,
            "aspect ratio {aspect} should be close to 5.0"
        );
    }

    #[test]
    fn downscale_handles_panoramic_image() {
        // Very large panorama, target depends on actual cell dimensions
        let (cell_w, cell_h) = get_cell_pixel_dimensions();
        let (max_w, max_h) = cells_to_pixels(80, 24, cell_w, cell_h);
        let img = make_test_image(16384, 4096);
        let result = downscale_to_display_pixels(&img, Some(80), Some(24));
        assert!(matches!(result, Cow::Owned(_)));
        assert!(result.width() <= max_w);
        assert!(result.height() <= max_h);
    }

    // =========================================================================
    // Tests for parse_video_dimensions
    // =========================================================================

    #[test]
    fn parse_dimensions_plain_csv() {
        assert_eq!(parse_video_dimensions("1920,1080").unwrap(), (1920, 1080));
    }

    #[test]
    fn parse_dimensions_trailing_newline() {
        assert_eq!(parse_video_dimensions("1920,1080\n").unwrap(), (1920, 1080));
    }

    #[test]
    fn parse_dimensions_handles_dolby_vision_trailing_comma() {
        // A Dolby Vision HEVC stream carries a DoVi configuration record as side
        // data. ffprobe's CSV writer appends an empty trailing field for that
        // nested section, yielding "3840,2160," — splitting only on the first
        // comma used to leave the height as "2160,", which failed to parse.
        assert_eq!(
            parse_video_dimensions("3840,2160,\n").unwrap(),
            (3840, 2160)
        );
    }

    #[test]
    fn parse_dimensions_handles_newline_separated() {
        // ffprobe's `default=noprint_wrappers=1:nokey=1` writer emits one value
        // per line; the parser must accept that layout too.
        assert_eq!(
            parse_video_dimensions("3840\n2160\n").unwrap(),
            (3840, 2160)
        );
    }

    #[test]
    fn parse_dimensions_errors_when_height_missing() {
        assert!(parse_video_dimensions("3840").is_err());
    }

    // =========================================================================
    // Tests for parse_fps_fraction (single frame-rate field)
    // =========================================================================

    #[test]
    fn parse_fps_fraction_ntsc() {
        assert!((parse_fps_fraction("30000/1001").unwrap() - 29.970_029_97).abs() < 1e-6);
    }

    #[test]
    fn parse_fps_fraction_integer() {
        assert_eq!(parse_fps_fraction("25/1\n"), Some(25.0));
    }

    #[test]
    fn parse_fps_fraction_handles_dolby_vision_trailing_comma() {
        // Same DoVi side-data trailing comma as the dimension probe:
        // "24000/1001,". Splitting the fraction left the denominator as
        // "1001,", which silently fell back to 1.0 — yielding 24000 fps
        // instead of 23.976.
        let fps = parse_fps_fraction("24000/1001,\n").unwrap();
        assert!((fps - 23.976_023_976).abs() < 1e-6, "got {fps}");
    }

    #[test]
    fn parse_fps_fraction_zero_denominator_is_none() {
        // ffprobe reports "0/0" for streams with an unknown frame rate; a naive
        // divide produces NaN, so this must be None and let the caller fall back.
        assert_eq!(parse_fps_fraction("0/0"), None);
    }

    #[test]
    fn parse_fps_fraction_empty_is_none() {
        assert_eq!(parse_fps_fraction(""), None);
    }

    // =========================================================================
    // Tests for parse_video_fps (avg/r selection)
    // =========================================================================

    #[test]
    fn parse_fps_empty_falls_back_to_default() {
        assert_eq!(parse_video_fps(""), DEFAULT_FPS);
    }

    #[test]
    fn parse_fps_both_unknown_falls_back_to_default() {
        assert_eq!(
            parse_video_fps("r_frame_rate=0/0\navg_frame_rate=0/0\n"),
            DEFAULT_FPS
        );
    }

    #[test]
    fn parse_fps_prefers_avg_frame_rate_over_r_frame_rate() {
        // A variable-frame-rate screen recording: r_frame_rate is the timebase
        // tick (287/12 ≈ 23.92 fps) while avg_frame_rate is the true average
        // (≈ 12.06 fps). Playback timing must use the average, or the video
        // plays roughly 2× too fast.
        let probe = "r_frame_rate=287/12\navg_frame_rate=60525000/5017157\n";
        let fps = parse_video_fps(probe);
        assert!((fps - 12.063).abs() < 0.01, "got {fps}");
    }

    #[test]
    fn parse_fps_falls_back_to_r_frame_rate_when_avg_unknown() {
        // Some containers report avg_frame_rate as "0/0" (unknown). Fall back to
        // r_frame_rate rather than the hard-coded default.
        let probe = "r_frame_rate=30/1\navg_frame_rate=0/0\n";
        assert_eq!(parse_video_fps(probe), 30.0);
    }

    // =========================================================================
    // Tests for ensure_file_exists
    // =========================================================================

    /// Build a path that is guaranteed not to exist, keyed on pid + nanos so
    /// concurrent test runs never collide on it (see CLAUDE.md parallel-safety).
    fn nonexistent_path() -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "ic-does-not-exist-{}-{nanos}.mp4",
            std::process::id()
        ))
    }

    #[test]
    fn ensure_file_exists_errors_on_missing_file() {
        let missing = nonexistent_path();
        let err = ensure_file_exists(&missing).expect_err("missing file should error");
        let message = err.to_string();
        assert!(
            message.contains("File not found"),
            "error should clearly say the file was not found, got: {message}"
        );
        assert!(
            message.contains(&missing.display().to_string()),
            "error should name the missing path, got: {message}"
        );
    }

    #[test]
    fn ensure_file_exists_errors_on_directory() {
        // A directory exists but cannot be displayed as a file. temp_dir() is
        // always present, so this is a stable, parallel-safe directory to test.
        let dir = std::env::temp_dir();
        let err = ensure_file_exists(&dir).expect_err("directory should error");
        let message = err.to_string();
        assert!(
            message.contains("directory"),
            "error should explain the path is a directory, got: {message}"
        );
    }

    #[test]
    fn ensure_file_exists_accepts_regular_file() {
        // The running test binary is a regular file that is guaranteed to exist.
        let real_file = std::env::current_exe().expect("current exe path");
        assert!(ensure_file_exists(&real_file).is_ok());
    }

    // =========================================================================
    // Tests for image_rows
    // =========================================================================

    #[test]
    fn image_rows_divides_an_exact_multiple() {
        assert_eq!(image_rows(100, 20), 5);
        assert_eq!(image_rows(20, 20), 1);
    }

    #[test]
    fn image_rows_rounds_up_a_partial_row() {
        // A partial row still uses a full row.
        assert_eq!(image_rows(101, 20), 6);
        assert_eq!(image_rows(21, 20), 2);
        assert_eq!(image_rows(1, 20), 1);
    }

    #[test]
    fn image_rows_never_gives_zero() {
        // A movement of zero rows means one row to a terminal, so the row count
        // must stay at 1 or more.
        assert_eq!(image_rows(0, 20), 1);
        assert_eq!(image_rows(0, 0), 1);
    }

    #[test]
    fn image_rows_survives_a_zero_cell_height() {
        // get_cell_pixel_dimensions divides the terminal pixel height by the row
        // count, so it can give 0. A cell height of 0 counts as 1 pixel.
        assert_eq!(image_rows(100, 0), 100);
    }

    // =========================================================================
    // Tests for image_rows_in_cells
    // =========================================================================

    /// The width of one character cell in the tests below, in pixels.
    const TEST_CELL_WIDTH_PX: u32 = 10;

    /// The height of one character cell in the tests below, in pixels.
    const TEST_CELL_HEIGHT_PX: u32 = 20;

    #[test]
    fn image_rows_in_cells_takes_a_given_height() {
        // Both protocols scale the image into the rows that ic asks for, so a
        // given height is the answer, with or without a width.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                Some(10),
                Some(5),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                Some(7),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            7
        );
    }

    #[test]
    fn image_rows_in_cells_keeps_the_aspect_ratio_of_a_width() {
        // A width of 10 cells is 100 pixels. The image is twice as wide as it
        // is tall, so it renders 50 pixels high, which is 3 rows of 20 pixels.
        assert_eq!(
            image_rows_in_cells(
                100,
                50,
                Some(10),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            3
        );
        // A width of 20 cells is 200 pixels, which doubles the image to 200
        // pixels high, which is 10 rows.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                Some(20),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            10
        );
    }

    #[test]
    fn image_rows_in_cells_falls_back_to_the_pixel_height() {
        // With no width and no height both protocols render the image at its
        // own pixel size.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
        // A partial row still uses a full row.
        assert_eq!(
            image_rows_in_cells(
                100,
                101,
                None,
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            6
        );
    }

    #[test]
    fn image_rows_in_cells_survives_a_zero_image_width() {
        // A width of zero pixels gives no scale factor, so the width-only case
        // falls back to the pixel height of the image.
        assert_eq!(
            image_rows_in_cells(
                0,
                100,
                Some(10),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            5
        );
    }

    #[test]
    fn image_rows_in_cells_never_gives_zero() {
        // A movement of zero rows means one row to a terminal, so the row count
        // must stay at 1 or more.
        assert_eq!(
            image_rows_in_cells(
                100,
                100,
                None,
                Some(0),
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            1
        );
        assert_eq!(
            image_rows_in_cells(100, 0, None, None, TEST_CELL_WIDTH_PX, TEST_CELL_HEIGHT_PX),
            1
        );
        assert_eq!(
            image_rows_in_cells(
                100,
                1,
                Some(1),
                None,
                TEST_CELL_WIDTH_PX,
                TEST_CELL_HEIGHT_PX
            ),
            1
        );
    }

    // =========================================================================
    // Tests for auto_fit_rows
    // =========================================================================

    #[test]
    fn auto_fit_rows_keeps_the_prompt_row_when_there_is_no_header() {
        // stdin and video frames print no header, so only the prompt row goes.
        assert_eq!(auto_fit_rows(24, HeaderRows(0)), 23);
    }

    #[test]
    fn auto_fit_rows_pays_for_a_one_row_header() {
        // A file argument prints the file name on one row above the image.
        assert_eq!(auto_fit_rows(24, HeaderRows(1)), 22);
    }

    #[test]
    fn auto_fit_rows_pays_for_a_two_row_header() {
        // The monitor mode prints an empty row and then the name of the image.
        assert_eq!(auto_fit_rows(24, HeaderRows(2)), 21);
    }

    #[test]
    fn auto_fit_rows_gives_at_least_one_row_in_a_short_terminal() {
        // A terminal too short for the header and the prompt still gets an
        // image of one row, instead of an image of zero rows.
        assert_eq!(auto_fit_rows(2, HeaderRows(1)), 1);
        assert_eq!(auto_fit_rows(1, HeaderRows(2)), 1);
        assert_eq!(auto_fit_rows(0, HeaderRows(0)), 1);
    }
}
