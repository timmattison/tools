use anyhow::{Context, Result};
use base64::prelude::*;
use clap::Parser;
use image::{DynamicImage, ImageFormat};
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
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

/// Fast terminal image display utility for iTerm2 and Kitty
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
/// - Other terminals: Limited or no image support
/// 
/// iTerm2 Settings for Better Performance:
/// - Disable "Background opacity" in Preferences > Profiles > Window
/// - Reduce scrollback buffer size in Preferences > Profiles > Terminal
#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
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
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Validate arguments
    if !args.stdin && args.file.is_none() {
        eprintln!("Error: Must specify a file or use --stdin");
        std::process::exit(1);
    }

    if args.stdin && args.file.is_some() {
        eprintln!("Error: Cannot specify both --stdin and a file");
        std::process::exit(1);
    }

    if let Some(width) = args.width {
        if width == 0 {
            eprintln!("Error: Width must be greater than 0");
            std::process::exit(1);
        }
    }

    if let Some(height) = args.height {
        if height == 0 {
            eprintln!("Error: Height must be greater than 0");
            std::process::exit(1);
        }
    }

    if args.scale == 0 || args.scale > 100 {
        eprintln!("Error: Scale must be between 1 and 100");
        std::process::exit(1);
    }

    if args.do_not_drop_frames && args.adaptive_fps {
        eprintln!("Warning: --do-not-drop-frames and --adaptive-fps are mutually exclusive.");
        std::process::exit(1);
    }

    // Show tmux warning if detected
    if std::env::var("TMUX").is_ok() {
        eprintln!("Warning: tmux detected. This utility does not work in tmux. Please run it directly in your terminal.");
    }

    if args.stdin {
        display_image_from_stdin(&args)?;
    } else if let Some(ref file_path) = args.file {
        if is_video_file(file_path) {
            display_video_from_file(file_path, &args)?;
        } else {
            display_image_from_file(file_path, &args)?;
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

fn display_video_from_file(file_path: &PathBuf, args: &Args) -> Result<()> {
    // Check if ffmpeg is available
    if let Err(_) = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
    {
        anyhow::bail!(
            "ffmpeg is required for video playback but was not found. Please install ffmpeg."
        );
    }

    // Clear screen initially
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

fn play_video_simple(
    file_path: &PathBuf,
    _frame_duration: Duration,
    args: &Args,
    duration: f64,
    mut fps: f64,
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

    // Set up raw mode for non-blocking input
    let _raw_mode = io::stdout()
        .into_raw_mode()
        .context("Failed to set terminal to raw mode")?;

    // Spawn a thread to handle keyboard input
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let _input_handle = thread::spawn(move || {
        let stdin = io::stdin();
        for key_result in stdin.keys() {
            if let Ok(key) = key_result {
                let control = match key {
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
                };

                if let Some(ctrl) = control {
                    let is_exit = matches!(ctrl, VideoControl::Exit);
                    let _ = input_tx.send(ctrl);
                    if is_exit {
                        break;
                    }
                }
            }
        }
    });

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
                "-ss",
                &format!("{:.3}", current_time), // Seek to current position
                "-i",
                file_path.to_str().unwrap(),
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
                                    // Move forward one frame (1/fps seconds)
                                    current_time += 1.0 / fps;
                                    if current_time >= duration {
                                        current_time = duration - (1.0 / fps); // Stay on last frame
                                    }
                                    show_frame_after_seek = true;
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::FrameBackward) => {
                                    // Move backward one frame (1/fps seconds)
                                    current_time -= 1.0 / fps;
                                    current_time = current_time.max(0.0);
                                    show_frame_after_seek = true;
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::SeekForward(seconds)) => {
                                    current_time += seconds;
                                    if current_time >= duration {
                                        current_time = duration - (1.0 / fps); // Stay on last frame
                                    }
                                    show_frame_after_seek = true;
                                    break; // Restart ffmpeg at new position
                                }
                                Ok(VideoControl::SeekBackward(seconds)) => {
                                    current_time -= seconds;
                                    current_time = current_time.max(0.0);
                                    show_frame_after_seek = true;
                                    break; // Restart ffmpeg at new position
                                }
                                _ => {}
                            }
                        }
                        break 'frame_loop; // Restart ffmpeg after any seeking
                    }
                }
                Ok(VideoControl::FrameForward) => {
                    // Pause and move forward one frame
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
                    // Pause and move backward one frame
                    is_paused = true;
                    show_frame_after_seek = true;
                    pause_start_time = Some(Instant::now());
                    current_time -= 1.0 / fps;
                    current_time = current_time.max(0.0);
                    let _ = ffmpeg_child.kill();
                    break 'frame_loop; // Restart ffmpeg at new position
                }
                Ok(VideoControl::SeekForward(seconds)) => {
                    // Pause and seek forward
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
                    // Pause and seek backward
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
            if current_time > expected_video_time {
                // We're ahead of schedule (video time > real time), wait
                let time_ahead = current_time - expected_video_time;
                thread::sleep(Duration::from_secs_f64(time_ahead));
            } else if current_time < expected_video_time && !args.do_not_drop_frames {
                // We're behind schedule - check if we should drop frames
                let time_behind = expected_video_time - current_time;
                let frames_behind = (time_behind * fps) as u32;

                if frames_behind > 1 {
                    // Skip frames to catch up
                    let frames_to_skip = frames_behind.min(5); // Don't skip too many at once
                    current_time += frames_to_skip as f64 / fps;

                    // Try to skip the frame data in ffmpeg output
                    let mut skip_buffer = vec![0u8; frame_size];
                    for _ in 0..frames_to_skip {
                        if reader.read_exact(&mut skip_buffer).is_err() {
                            break;
                        }
                    }
                    continue;
                }
            }

            // Try to read next frame
            match reader.read_exact(&mut frame_buffer) {
                Ok(()) => {
                    // Successfully read a frame

                    // Convert RGB data to image
                    if let Ok(img) = rgb_data_to_image(&frame_buffer, video_width, video_height) {
                        // Periodic memory cleanup - clear scrollback to prevent memory buildup
                        frames_since_clear += 1;
                        let cleanup_frequency = if fps > 30.0 {
                            // More frequent cleanup for high FPS videos to manage memory
                            args.memory_cleanup_frequency.min(30)
                        } else {
                            args.memory_cleanup_frequency
                        };

                        if frames_since_clear >= cleanup_frequency {
                            clear_scrollback()?;
                            frames_since_clear = 0;
                        }

                        // Check terminal size and decide on clearing strategy
                        let current_terminal_size = get_terminal_size().ok();
                        let should_clear_screen = if first_frame {
                            // Always clear for first frame
                            first_frame = false;
                            true
                        } else if let (Some(current), Some(previous)) =
                            (current_terminal_size, previous_terminal_size)
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
                        previous_terminal_size = current_terminal_size;

                        // Adaptive FPS monitoring
                        if args.adaptive_fps {
                            let display_time = last_display_time.elapsed();
                            let expected_frame_time = Duration::from_secs_f64(1.0 / fps);

                            if display_time > expected_frame_time * 2 {
                                // Frame took more than 2x the expected time - we're falling behind
                                consecutive_late_frames += 1;

                                if consecutive_late_frames >= 5 && !adaptive_fps_active {
                                    // Reduce FPS to help terminal keep up
                                    fps = (fps * 0.75).max(10.0); // Don't go below 10 FPS
                                    adaptive_fps_active = true;
                                    // eprintln!("Warning: Terminal falling behind, reducing playback rate to {:.1} FPS", fps);
                                }
                            } else {
                                consecutive_late_frames = 0;

                                // If we've been adaptive and frames are smooth, gradually increase FPS
                                if adaptive_fps_active && consecutive_late_frames == 0 {
                                    fps = (fps * 1.05).min(original_fps);
                                    if fps >= original_fps * 0.95 {
                                        fps = original_fps;
                                        adaptive_fps_active = false;
                                    }
                                }
                            }

                            last_display_time = Instant::now();
                        }

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
    // Check if ffprobe is available
    if let Err(_) = std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
    {
        anyhow::bail!(
            "ffprobe is required for video playback but was not found. Please install ffmpeg."
        );
    }

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
    // Check if ffprobe is available
    if let Err(_) = std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
    {
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
    // Check if ffprobe is available
    if let Err(_) = std::process::Command::new("ffprobe")
        .arg("-version")
        .output()
    {
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
    print!("\x1b[2J\x1b[H"); // Clear screen and move cursor to top-left
    io::stdout().flush().context("Failed to flush output")?;
    Ok(())
}

fn clear_scrollback() -> Result<()> {
    // Clear iTerm2 scrollback buffer to free memory
    print!("\x1b]1337;ClearScrollback\x07");
    io::stdout()
        .flush()
        .context("Failed to flush scrollback clear")?;
    Ok(())
}

fn move_cursor_home() -> Result<()> {
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

fn display_image(mut img: DynamicImage, args: &Args) -> Result<()> {
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

    // Apply scaling for performance optimization
    let (scaled_width, scaled_height) = if args.scale < 100 {
        let scale_factor = args.scale as f32 / 100.0;
        (
            target_width.map(|w| ((w as f32 * scale_factor) as u32).max(1)),
            target_height.map(|h| ((h as f32 * scale_factor) as u32).max(1)),
        )
    } else {
        (target_width, target_height)
    };

    // Since we're always using character-based sizing, we don't resize the image anymore -
    // iTerm2 will handle the sizing based on the character dimensions we provide
    // This avoids quality loss from resizing
    let needs_resize = false;

    if needs_resize {
        if let (Some(width), Some(height)) = (target_width, target_height) {
            img = if args.preserve_aspect {
                img.resize(width, height, image::imageops::FilterType::Triangle)
            } else {
                img.resize_exact(width, height, image::imageops::FilterType::Triangle)
            };
        } else if let Some(width) = target_width {
            let height = (img.height() * width) / img.width();
            img = img.resize(width, height, image::imageops::FilterType::Triangle);
        } else if let Some(height) = target_height {
            let width = (img.width() * height) / img.height();
            img = img.resize(width, height, image::imageops::FilterType::Triangle);
        }
    }

    // Convert image to the specified format for encoding
    let mut encoded_data = Vec::with_capacity(img.width() as usize * img.height() as usize * 4);

    // PNM is uncompressed by default and has no compression options at all
    img.write_to(
        &mut io::Cursor::new(&mut encoded_data),
        ImageFormat::Pnm,
    )
    .context("Failed to encode image as PNM")?;

    // Detect terminal type and use appropriate graphics protocol
    if is_kitty_terminal() {
        // For Kitty, use RGB data directly without encoding as PNM
        let rgb_data = img.to_rgb8();
        print_kitty_image(
            rgb_data.as_raw(),
            img.width(),
            img.height(),
            scaled_width,
            scaled_height,
            args.no_newline,
        )?;
    } else {
        // Base64 encode the image data with pre-allocated capacity
        let encoded = BASE64_STANDARD.encode(&encoded_data);

        // Use iTerm2 protocol for other terminals
        print_iterm2_image_with_chars(&encoded, scaled_width, scaled_height, args.no_newline)?;
    }

    Ok(())
}

fn is_kitty_terminal() -> bool {
    // Check if we're running in Kitty terminal
    std::env::var("KITTY_WINDOW_ID").is_ok() 
        || std::env::var("TERM").map_or(false, |term| term.contains("kitty"))
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
        write!(stdout, "\n")?;
    }

    stdout.flush().context("Failed to flush output")?;
    Ok(())
}

fn print_iterm2_image_with_chars(
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
