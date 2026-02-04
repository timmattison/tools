use anyhow::{Context, Result};
use base64::prelude::*;
use buildinfo::version_string;
use clap::Parser;
use image::{DynamicImage};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use terminal_size::{terminal_size, Height, Width};
use termion::event::Key;
use termion::input::TermRead;
use termion::raw::IntoRawMode;

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
/// - WezTerm: Uses iTerm2 inline image protocol
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
    /// Image or video file to display
    #[clap(value_name = "FILE")]
    file: Option<PathBuf>,

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
}

fn main() -> Result<()> {
    let args = Args::parse();

    validate_arguments(&args)?;

    if args.stdin {
        display_image_from_stdin(&args)?;
    } else if let Some(ref file_path) = args.file {
        if is_video_file(file_path) {
            display_video_from_file(file_path, &args)?;
        } else if is_image_file(file_path) {
            display_image_from_file(file_path, &args)?;
        } else {
            // Treat as text file
            display_text_file(file_path)?;
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
    let input_modes = [args.stdin, args.file.is_some(), !args.monitor.is_empty()];
    let input_count = input_modes.iter().filter(|&&x| x).count();
    
    if input_count == 0 {
        anyhow::bail!("Must specify a file, use --stdin, or use --monitor");
    }
    
    if input_count > 1 {
        anyhow::bail!("Cannot specify multiple input modes (--stdin, file, --monitor)");
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
                                        println!("\nFound new image: {}", path.display());
                                        match display_image_from_file(&path, args) {
                                            Ok(_) => {
                                                recent_files.insert(path.clone());
                                            },
                                            Err(e) => {
                                                eprintln!("Failed to display image {}: {}", path.display(), e);
                                            }
                                        }
                                    } else if is_text_file(&path) {
                                        println!("\nFound new text file: {}", path.display());
                                        match display_text_file(&path) {
                                            Ok(_) => {
                                                recent_files.insert(path.clone());
                                            },
                                            Err(e) => {
                                                eprintln!("Failed to display text file {}: {}", path.display(), e);
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



fn is_video_file(file_path: &PathBuf) -> bool {
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

fn is_image_file(file_path: &PathBuf) -> bool {
    if let Some(extension) = file_path.extension() {
        if let Some(ext_str) = extension.to_str() {
            let ext_lower = ext_str.to_lowercase();
            matches!(
                ext_lower.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tiff" | "tif" | "webp" | "svg" | "ico" | "ppm" | "pbm" | "pgm" | "pnm"
            )
        } else {
            false
        }
    } else {
        false
    }
}

fn is_text_file(file_path: &PathBuf) -> bool {
    // If it's not an image or video file, treat it as text
    !is_image_file(file_path) && !is_video_file(file_path)
}

fn ensure_ffmpeg_available() -> Result<()> {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        anyhow::bail!("ffmpeg is required for video playback but was not found. Please install ffmpeg.");
    }
    Ok(())
}

fn ensure_ffprobe_available() -> Result<()> {
    if std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
        .is_err()
    {
        anyhow::bail!("ffprobe is required for video playback but was not found. Please install ffmpeg.");
    }
    Ok(())
}

fn validate_terminal_for_graphics(terminal_caps: &TerminalCapabilities, feature: &str) -> Result<()> {
    // Check for tmux first, since graphics don't work in tmux
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
                • WezTerm - supports iTerm2 image protocol\n\
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
                • WezTerm - supports iTerm2 image protocol\n\
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

fn display_video_from_file(file_path: &PathBuf, args: &Args) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    
    validate_terminal_for_graphics(&terminal_caps, "Video")?;
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

fn setup_video_controls(terminal_caps: &TerminalCapabilities) -> Result<(Option<termion::raw::RawTerminal<io::Stdout>>, std::sync::mpsc::Receiver<VideoControl>, Option<thread::JoinHandle<()>>)> {
    let supports_interactive_controls = terminal_caps.supports_raw_mode;

    if !supports_interactive_controls {
        print_control_notice(terminal_caps);
        let (_, rx) = std::sync::mpsc::channel();
        return Ok((None, rx, None));
    }

    // Set up raw mode for non-blocking input
    let raw_mode = match io::stdout().into_raw_mode() {
        Ok(raw_mode) => Some(raw_mode),
        Err(_) => None,
    };

    // Spawn a thread to handle keyboard input
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let input_handle = if raw_mode.is_some() {
        Some(thread::spawn(move || {
            let stdin = io::stdin();
            for key_result in stdin.keys() {
                if let Ok(key) = key_result {
                    let control = map_key_to_control(key);
                    if let Some(ctrl) = control {
                        let is_exit = matches!(ctrl, VideoControl::Exit);
                        let _ = input_tx.send(ctrl);
                        if is_exit {
                            break;
                        }
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
            eprintln!("Notice: Running in Alacritty. Video will play without interactive controls.");
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
        // We're behind schedule - check if we should drop frames
        let time_behind = expected_video_time - current_time;
        let frames_behind = (time_behind * fps) as u32;

        if frames_behind > 1 {
            // Skip frames to catch up
            let frames_to_skip = frames_behind.min(5); // Don't skip too many at once
            let new_time = current_time + frames_to_skip as f64 / fps;

            // Try to skip the frame data in ffmpeg output
            let mut skip_buffer = vec![0u8; frame_size];
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
    } else if let (Some(current), Some(previous)) =
        (current_terminal_size, *previous_terminal_size)
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

    display_image(img, args)?;

    // Draw progress bar
    if let Some((term_width, term_height)) = current_terminal_size {
        draw_progress_bar(
            current_time,
            duration,
            fps,
            term_width,
            term_height,
        )?;
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

fn play_video_simple(
    file_path: &PathBuf,
    _frame_duration: Duration,
    args: &Args,
    duration: f64,
    mut fps: f64,
) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    let (_raw_mode, input_rx, _input_handle) = setup_video_controls(&terminal_caps)?;
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
            .args(&[
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
        let mut frame_buffer = vec![0u8; frame_size];

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

    // Clean up the input thread
    drop(_input_handle);

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

fn get_video_dimensions(file_path: &PathBuf) -> Result<(u32, u32)> {
    ensure_ffprobe_available()?;

    // Use ffprobe to get video dimensions
    let output = std::process::Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
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

    let dimensions_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Parse "width,height" format
    if let Some(comma_pos) = dimensions_str.find(',') {
        let width: u32 = dimensions_str[..comma_pos]
            .parse()
            .context("Failed to parse video width")?;
        let height: u32 = dimensions_str[comma_pos + 1..]
            .parse()
            .context("Failed to parse video height")?;
        Ok((width, height))
    } else {
        anyhow::bail!("Invalid dimensions format from ffprobe: {}", dimensions_str);
    }
}

fn get_video_fps(file_path: &PathBuf) -> Result<f64> {
    if ensure_ffprobe_available().is_err() {
        eprintln!("Warning: ffprobe not found, using default 24 fps");
        return Ok(24.0);
    }

    // Use ffprobe to get video frame rate
    let output = std::process::Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate",
            "-of",
            "csv=p=0",
            file_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        return Ok(24.0); // Default to 24 fps
    }

    let fps_str = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Parse fraction like "24/1" or "30000/1001"
    if let Some(slash_pos) = fps_str.find('/') {
        let numerator: f64 = fps_str[..slash_pos].parse().unwrap_or(24.0);
        let denominator: f64 = fps_str[slash_pos + 1..].parse().unwrap_or(1.0);
        Ok(numerator / denominator)
    } else {
        Ok(fps_str.parse().unwrap_or(24.0))
    }
}

fn get_video_duration(file_path: &PathBuf) -> Result<f64> {
    if ensure_ffprobe_available().is_err() {
        eprintln!("Warning: ffprobe not found, using default duration");
        return Ok(60.0); // Default to 60 seconds
    }

    // Use ffprobe to get video duration in seconds
    let output = std::process::Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
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
        (current_time / total_duration).min(1.0).max(0.0)
    } else {
        0.0
    };

    // Format time strings with frame numbers
    let current_min = (current_time / 60.0) as u32;
    let current_sec = (current_time % 60.0) as u32;
    let current_frame = ((current_time % 1.0) * fps) as u32;

    let total_min = (total_duration / 60.0) as u32;
    let total_sec = (total_duration % 60.0) as u32;
    let total_frame = ((total_duration % 1.0) * fps) as u32;

    let time_str = format!(
        "{:02}:{:02}:{:02} / {:02}:{:02}:{:02}",
        current_min, current_sec, current_frame, total_min, total_sec, total_frame
    );

    // Calculate available width for progress bar (leave space for time and brackets)
    let time_len = time_str.len() as u32;
    let available_width = terminal_width.saturating_sub(time_len + 4); // 4 chars for " [" and "] "

    if available_width > 0 {
        let filled_chars = (progress * available_width as f64) as u32;
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

fn display_image_from_file(file_path: &PathBuf, args: &Args) -> Result<()> {
    let img = image::open(file_path)
        .with_context(|| format!("Failed to open image file: {}", file_path.display()))?;

    display_image(img, args)
}

fn display_text_file(file_path: &PathBuf) -> Result<()> {
    let contents = fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read text file: {}", file_path.display()))?;
    
    print!("{}", contents);
    io::stdout()
        .flush()
        .context("Failed to flush output")?;
    
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

    display_image(img, args)
}

fn display_image(img: DynamicImage, args: &Args) -> Result<()> {
    let terminal_caps = detect_terminal_capabilities();
    
    validate_terminal_for_graphics(&terminal_caps, "Image")?;
    // Always use character-based sizing (fit mode), but respect user-specified dimensions if provided
    let (target_width, target_height) = if args.width.is_some() || args.height.is_some() {
        // Use user-specified dimensions but still use character-based sizing
        (args.width, args.height)
    } else {
        // Auto-fit to terminal window size
        let (term_width, term_height) = get_terminal_size()?;
        // Leave some margin for terminal chrome and preserve one line for prompt
        let safe_width = if term_width > 4 {
            term_width - 2
        } else {
            term_width
        };
        let safe_height = if term_height > 2 {
            term_height - 1
        } else {
            term_height
        };
        (Some(safe_width), Some(safe_height))
    };

    // Apply scaling
    let (scaled_width, scaled_height) = if args.scale < 100 {
        let scale_factor = args.scale as f32 / 100.0;
        (
            target_width.map(|w| ((w as f32 * scale_factor) as u32).max(1)),
            target_height.map(|h| ((h as f32 * scale_factor) as u32).max(1)),
        )
    } else {
        (target_width, target_height)
    };

    // Choose optimal display method based on terminal capabilities
    // Kitty and Ghostty both support the Kitty graphics protocol
    if is_kitty_graphics_terminal() {
        display_image_kitty(&img, scaled_width, scaled_height, args)
    } else {
        display_image_iterm2(&img, scaled_width, scaled_height, args)
    }
}

/// Calculate display dimensions that preserve aspect ratio within the given bounds
/// Terminal cells are typically ~2:1 (height:width in pixels), so we account for that
fn calculate_aspect_preserving_size(
    img_width: u32,
    img_height: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
    preserve_aspect: bool,
) -> (Option<u32>, Option<u32>) {
    if !preserve_aspect {
        return (max_width, max_height);
    }

    match (max_width, max_height) {
        (Some(max_w), Some(max_h)) => {
            // Terminal cells are roughly 2:1 (height:width in pixels)
            // So 1 row ≈ 2 columns worth of pixels
            // Adjust the max_height to account for cell aspect ratio
            let cell_aspect_ratio = 2.0;
            let effective_max_h_pixels = max_h as f64 * cell_aspect_ratio;
            let effective_max_w_pixels = max_w as f64;

            let img_aspect = img_width as f64 / img_height as f64;
            let box_aspect = effective_max_w_pixels / effective_max_h_pixels;

            if img_aspect > box_aspect {
                // Image is wider than box - constrain by width
                let display_width = max_w;
                let display_height =
                    ((max_w as f64 / img_aspect) / cell_aspect_ratio).round() as u32;
                (Some(display_width), Some(display_height.max(1)))
            } else {
                // Image is taller than box - constrain by height
                let display_height = max_h;
                let display_width =
                    (max_h as f64 * cell_aspect_ratio * img_aspect).round() as u32;
                (Some(display_width.max(1)), Some(display_height))
            }
        }
        (Some(w), None) => (Some(w), None),
        (None, Some(h)) => (None, Some(h)),
        (None, None) => (None, None),
    }
}

/// Kitty terminal display with better performance for video
fn display_image_kitty(img: &DynamicImage, width: Option<u32>, height: Option<u32>, args: &Args) -> Result<()> {
    // Use RGB data directly for better performance (no base64 encoding overhead)
    let rgb_data = img.to_rgb8();

    // Calculate display dimensions that preserve aspect ratio
    // Terminal cells are typically ~2:1 (height:width in pixels), so we account for that
    let (display_width, display_height) = calculate_aspect_preserving_size(
        img.width(),
        img.height(),
        width,
        height,
        args.preserve_aspect,
    );

    // Use Kitty's more efficient graphics protocol with optimizations
    print_kitty_image(
        rgb_data.as_raw(),
        img.width(),
        img.height(),
        display_width,
        display_height,
        args.no_newline,
    )
}

/// Optimized iTerm2 display with reduced overhead
fn display_image_iterm2(img: &DynamicImage, width: Option<u32>, height: Option<u32>, args: &Args) -> Result<()> {
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

    print_iterm2_image(&encoded, width, height, args.no_newline)
}

/// Optimized Kitty image printing with reduced protocol overhead
#[derive(Debug, Clone)]
enum TerminalType {
    Kitty,
    Ghostty,
    ITerm2,
    WezTerm,
    Alacritty,
    Unknown,
}

#[derive(Debug, Clone)]
struct TerminalCapabilities {
    terminal_type: TerminalType,
    supports_graphics: bool,
    supports_raw_mode: bool,
}

fn detect_terminal_capabilities() -> TerminalCapabilities {
    let term = std::env::var("TERM").unwrap_or_default();
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();

    let terminal_type = if std::env::var("KITTY_WINDOW_ID").is_ok() || term.contains("kitty") {
        TerminalType::Kitty
    } else if term_program == "ghostty"
        || std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || term.contains("ghostty")
    {
        TerminalType::Ghostty
    } else if term_program.contains("iTerm") || std::env::var("ITERM_SESSION_ID").is_ok() {
        TerminalType::ITerm2
    } else if term_program.contains("WezTerm") {
        TerminalType::WezTerm
    } else if std::env::var("ALACRITTY_SOCKET").is_ok() || term.contains("alacritty") {
        TerminalType::Alacritty
    } else {
        TerminalType::Unknown
    };

    let supports_graphics = !matches!(terminal_type, TerminalType::Alacritty) &&
        !term.contains("linux") &&  // Linux console doesn't support graphics
        !term.contains("screen") &&  // Screen doesn't support graphics
        !term.starts_with("vt");    // VT terminals don't support graphics

    let supports_raw_mode = {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::isatty(io::stdout().as_raw_fd()) == 1
        }
    };

    TerminalCapabilities {
        terminal_type,
        supports_graphics,
        supports_raw_mode,
    }
}

fn is_kitty_graphics_terminal() -> bool {
    matches!(
        detect_terminal_capabilities().terminal_type,
        TerminalType::Kitty | TerminalType::Ghostty
    )
}

fn print_kitty_image(
    rgb_data: &[u8],
    img_width: u32,
    img_height: u32,
    display_width: Option<u32>,
    display_height: Option<u32>,
    no_newline: bool,
) -> Result<()> {
    let mut stdout = io::stdout().lock();

    // Kitty graphics protocol format:
    // ESC _ G <key>=<value>,<key>=<value>,... ; <base64_data> ESC \
    // For large images, we need to chunk the data

    let base64_data = BASE64_STANDARD.encode(rgb_data);
    let chunk_size = 4096; // Kitty recommended chunk size

    if base64_data.len() <= chunk_size {
        // Small image, send in one chunk
        write!(stdout, "\x1b_Ga=T,f=24,s={},v={}", img_width, img_height)?;

        // Add display size if specified (in character cells)
        if let Some(w) = display_width {
            write!(stdout, ",c={}", w)?;
        }
        if let Some(h) = display_height {
            write!(stdout, ",r={}", h)?;
        }

        write!(stdout, ";{}\x1b\\", base64_data)?;
    } else {
        // Large image, send in chunks
        let chunks: Vec<&str> = base64_data
            .as_bytes()
            .chunks(chunk_size)
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect();

        for (i, chunk) in chunks.iter().enumerate() {
            if i == 0 {
                // First chunk
                write!(stdout, "\x1b_Ga=T,f=24,s={},v={}", img_width, img_height)?;

                // Add display size if specified (in character cells)
                if let Some(w) = display_width {
                    write!(stdout, ",c={}", w)?;
                }
                if let Some(h) = display_height {
                    write!(stdout, ",r={}", h)?;
                }

                write!(stdout, ",m=1;{}\x1b\\", chunk)?;
            } else if i == chunks.len() - 1 {
                // Last chunk
                write!(stdout, "\x1b_Gm=0;{}\x1b\\", chunk)?;
            } else {
                // Middle chunk
                write!(stdout, "\x1b_Gm=1;{}\x1b\\", chunk)?;
            }
        }
    }

    if !no_newline {
        writeln!(stdout)?;
    }

    stdout.flush().context("Failed to flush output")?;
    Ok(())
}

/// Optimized iTerm2 image printing with reduced protocol overhead
fn print_iterm2_image(
    base64_data: &str,
    width: Option<u32>,
    height: Option<u32>,
    no_newline: bool,
) -> Result<()> {
    let mut stdout = io::stdout().lock();

    // iTerm2 inline image protocol
    // ESC ] 1337 ; File = [arguments] : base64_data BEL
    write!(stdout, "\x1b]1337;File=inline=1")?;

    // Add width and height in character units (without 'px' suffix)
    if let Some(w) = width {
        write!(stdout, ";width={}", w)?;
    }
    if let Some(h) = height {
        write!(stdout, ";height={}", h)?;
    }

    // Preserve aspect ratio when fitting to terminal
    write!(stdout, ";preserveAspectRatio=1")?;

    // Don't move cursor after image to prevent scrollback accumulation
    write!(stdout, ";doNotMoveCursor=1")?;

    if no_newline {
        write!(stdout, ":{}\x07", base64_data)?;
    } else {
        write!(stdout, ":{}\x07\n", base64_data)?;
    }

    stdout.flush().context("Failed to flush output")?;
    Ok(())
}

