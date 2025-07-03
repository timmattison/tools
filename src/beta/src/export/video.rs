use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, RgbImage};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::Command;
use imageproc::drawing::draw_filled_rect_mut;
use imageproc::rect::Rect;
use crate::{Recording, EventType};
use super::terminal_renderer::{TerminalTheme, TerminalState};

// For now, we'll create a simple built-in font fallback
// This can be enhanced with an embedded font file later

pub async fn export_video(
    input: PathBuf,
    output: Option<PathBuf>,
    fps: u32,
    resolution: Option<String>,
    theme: String,
    optimize_web: bool,
) -> Result<()> {
    let file = File::open(&input)
        .context("Failed to open recording file")?;
    
    let recording: Recording = if input.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".gz"))
        .unwrap_or(false)
    {
        let reader = BufReader::new(file);
        let decoder = flate2::read::GzDecoder::new(reader);
        serde_json::from_reader(decoder)
            .context("Failed to parse compressed recording")?
    } else {
        let reader = BufReader::new(file);
        serde_json::from_reader(reader)
            .context("Failed to parse recording")?
    };
    
    let output_path = output.unwrap_or_else(|| {
        let mut path = input.clone();
        path.set_extension("mp4");
        path
    });
    
    let (width, height) = parse_resolution(&resolution, &recording)?;
    let terminal_theme = TerminalTheme::from_name(&theme);
    
    println!("Generating video frames...");
    println!("Resolution: {}x{}", width, height);
    println!("FPS: {}", fps);
    println!("Theme: {}", theme);
    
    let frames = generate_frames(&recording, width, height, fps, terminal_theme)?;
    
    println!("Generated {} frames", frames.len());
    println!("Encoding video with FFmpeg...");
    
    encode_video(&frames, &output_path, fps, optimize_web)?;
    
    println!("Video export saved to: {}", output_path.display());
    println!("Duration: {:.1}s", recording.duration);
    println!("Frames: {}", frames.len());
    
    Ok(())
}

fn parse_resolution(resolution: &Option<String>, recording: &Recording) -> Result<(u32, u32)> {
    if let Some(res) = resolution {
        let parts: Vec<&str> = res.split('x').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid resolution format. Use WIDTHxHEIGHT (e.g., 1920x1080)");
        }
        
        let width = parts[0].parse::<u32>()
            .context("Invalid width in resolution")?;
        let height = parts[1].parse::<u32>()
            .context("Invalid height in resolution")?;
        
        Ok((width, height))
    } else {
        let char_width = 12;
        let char_height = 20;
        let padding = 40;
        
        let width = (recording.width as u32 * char_width) + (padding * 2);
        let height = (recording.height as u32 * char_height) + (padding * 2);
        
        Ok((width, height))
    }
}

fn generate_frames(
    recording: &Recording,
    width: u32,
    height: u32,
    fps: u32,
    theme: TerminalTheme,
) -> Result<Vec<RgbImage>> {
    let mut frames = Vec::new();
    let frame_duration = 1.0 / fps as f64;
    let total_frames = (recording.duration * fps as f64).ceil() as usize;
    
    let mut terminal_state = TerminalState::new(
        recording.width as usize,
        recording.height as usize,
        theme.clone(),
    );
    
    let mut event_index = 0;
    
    for frame_num in 0..total_frames {
        let current_time = frame_num as f64 * frame_duration;
        
        while event_index < recording.events.len() && recording.events[event_index].time <= current_time {
            let event = &recording.events[event_index];
            if matches!(event.event_type, EventType::Output) {
                terminal_state.process_output(&event.data)
                    .context("Failed to process terminal output")?;
            }
            event_index += 1;
        }
        
        let frame = render_terminal_to_image(&terminal_state, width, height)?;
        frames.push(frame);
        
        if frame_num % 30 == 0 {
            println!("Generated frame {} / {}", frame_num + 1, total_frames);
        }
    }
    
    Ok(frames)
}

fn render_terminal_to_image(
    terminal_state: &TerminalState,
    width: u32,
    height: u32,
) -> Result<RgbImage> {
    let mut image = ImageBuffer::new(width, height);
    let theme = terminal_state.get_theme();
    
    // Fill background
    let bg_color = Rgb([theme.background.0, theme.background.1, theme.background.2]);
    for pixel in image.pixels_mut() {
        *pixel = bg_color;
    }
    
    let char_width = 12.0;
    let char_height = 20.0;
    let padding_x = 20.0;
    let padding_y = 20.0;
    
    let grid = terminal_state.get_grid();
    
    // Create a simple fallback font representation using rectangles
    // This will be replaced with proper font rendering once we have a font
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            let pixel_x = padding_x + (x as f32 * char_width);
            let pixel_y = padding_y + (y as f32 * char_height);
            
            // Draw background color for this cell
            let bg_rect = Rect::at(pixel_x as i32, pixel_y as i32)
                .of_size(char_width as u32, char_height as u32);
            
            draw_filled_rect_mut(
                &mut image,
                bg_rect,
                Rgb([cell.bg_color.0, cell.bg_color.1, cell.bg_color.2]),
            );
            
            // If character is not space, draw a simplified representation
            if cell.ch != ' ' && cell.ch != '\x00' {
                // For now, render characters as smaller rectangles with proper colors
                // This gives us better visual distinction than solid blocks
                let char_rect = if cell.bold {
                    // Bold characters are slightly larger
                    Rect::at((pixel_x + 2.0) as i32, (pixel_y + 3.0) as i32)
                        .of_size((char_width - 4.0) as u32, (char_height - 6.0) as u32)
                } else {
                    // Normal characters
                    Rect::at((pixel_x + 3.0) as i32, (pixel_y + 4.0) as i32)
                        .of_size((char_width - 6.0) as u32, (char_height - 8.0) as u32)
                };
                
                draw_filled_rect_mut(
                    &mut image,
                    char_rect,
                    Rgb([cell.fg_color.0, cell.fg_color.1, cell.fg_color.2]),
                );
                
                // Add underline if needed
                if cell.underline {
                    let underline_rect = Rect::at(pixel_x as i32, (pixel_y + char_height - 2.0) as i32)
                        .of_size(char_width as u32, 1);
                    
                    draw_filled_rect_mut(
                        &mut image,
                        underline_rect,
                        Rgb([cell.fg_color.0, cell.fg_color.1, cell.fg_color.2]),
                    );
                }
            }
        }
    }
    
    Ok(image)
}


fn encode_video(
    frames: &[RgbImage],
    output_path: &PathBuf,
    fps: u32,
    optimize_web: bool,
) -> Result<()> {
    let temp_dir = std::env::temp_dir().join("beta_video_export");
    std::fs::create_dir_all(&temp_dir)
        .context("Failed to create temp directory")?;
    
    for (i, frame) in frames.iter().enumerate() {
        let frame_path = temp_dir.join(format!("frame_{:06}.png", i));
        frame.save(&frame_path)
            .context("Failed to save frame")?;
    }
    
    let is_gif = output_path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_lowercase() == "gif")
        .unwrap_or(false);
    
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-y")
        .arg("-framerate")
        .arg(fps.to_string())
        .arg("-i")
        .arg(temp_dir.join("frame_%06d.png"))
        .arg("-r")
        .arg(fps.to_string());
    
    if is_gif {
        cmd.arg("-vf")
            .arg("fps=15,scale=-1:-1:flags=lanczos,split[s0][s1];[s0]palettegen=max_colors=256:stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle");
    } else {
        cmd.arg("-c:v")
            .arg("libx264")
            .arg("-pix_fmt")
            .arg("yuv420p");
        
        if optimize_web {
            cmd.arg("-crf")
                .arg("23")
                .arg("-preset")
                .arg("medium")
                .arg("-movflags")
                .arg("faststart");
        }
    }
    
    cmd.arg(output_path);
    
    let output = cmd.output()
        .context("Failed to run FFmpeg")?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("FFmpeg failed: {}", stderr);
    }
    
    std::fs::remove_dir_all(&temp_dir).ok();
    
    Ok(())
}