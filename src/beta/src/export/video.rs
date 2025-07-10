use anyhow::{Context, Result};
use image::{ImageBuffer, Rgb, RgbImage};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::process::Command;
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut};
use imageproc::rect::Rect;
use ab_glyph::{FontRef, PxScale, Font};
use std::collections::HashMap;
use crate::{Recording, EventType};
use super::terminal_renderer::{TerminalTheme, TerminalState};

struct FontManager {
    fonts: Vec<FontRef<'static>>,
    glyph_cache: HashMap<char, usize>, // Character -> font index
}

impl FontManager {
    fn new() -> Result<Self> {
        let mut fonts = Vec::new();
        let font_paths = get_font_paths();
        
        for font_path in &font_paths {
            if let Ok(font) = load_font_from_path(font_path) {
                fonts.push(font);
                if fonts.len() >= 5 { // Limit to reasonable number of fonts
                    break;
                }
            }
        }
        
        if fonts.is_empty() {
            return Err(anyhow::anyhow!("No fonts could be loaded"));
        }
        
        Ok(FontManager {
            fonts,
            glyph_cache: HashMap::new(),
        })
    }
    
    fn get_best_font_for_char(&mut self, ch: char) -> &FontRef<'static> {
        // Check cache first
        if let Some(&font_index) = self.glyph_cache.get(&ch) {
            return &self.fonts[font_index];
        }
        
        // Find font that contains this character
        for (i, font) in self.fonts.iter().enumerate() {
            if font.glyph_id(ch).0 != 0 { // 0 means missing glyph
                self.glyph_cache.insert(ch, i);
                return &self.fonts[i];
            }
        }
        
        // Fallback to first font if no font contains the character
        &self.fonts[0]
    }
}

fn get_font_paths() -> Vec<&'static str> {
    vec![
        // User's preferred font - JetBrains Mono Nerd Font Medium
        "~/Library/Fonts/JetBrainsMonoNerdFontMono-Medium.ttf",
        "~/Library/Fonts/JetBrains Mono Nerd Font Mono Medium.ttf", // Space variant
        "/Library/Fonts/JetBrainsMonoNerdFontMono-Medium.ttf",
        "/Library/Fonts/JetBrains Mono Nerd Font Mono Medium.ttf",
        
        // User's fallback - Monaco
        "/System/Library/Fonts/Monaco.ttf",
        
        // Unicode-rich fonts for better coverage
        "/System/Library/Fonts/Apple Braille Outline 6 Dot.ttf", // macOS Braille
        "/System/Library/Fonts/Apple Braille Outline 8 Dot.ttf", // macOS Braille
        "/System/Library/Fonts/Apple Braille Pinpoint 6 Dot.ttf", // macOS Braille
        "/System/Library/Fonts/Apple Braille Pinpoint 8 Dot.ttf", // macOS Braille
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", // Linux - good Unicode
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf", // Linux (alternative)
        "/usr/share/fonts/truetype/unifont/unifont.ttf", // Linux - comprehensive Unicode
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf", // Linux
        
        // Other JetBrains Mono Nerd Font variants - macOS user fonts
        "~/Library/Fonts/JetBrainsMono Nerd Font Mono.ttf",
        "~/Library/Fonts/JetBrainsMonoNL Nerd Font Mono.ttf", // No Ligatures
        "~/Library/Fonts/JetBrains Mono Nerd Font Regular.ttf",
        
        // JetBrains Mono Nerd Font - macOS system fonts
        "/Library/Fonts/JetBrainsMono Nerd Font Mono.ttf",
        "/Library/Fonts/JetBrainsMonoNL Nerd Font Mono.ttf",
        
        // JetBrains Mono Nerd Font - Linux user fonts
        "~/.local/share/fonts/JetBrainsMonoNerdFont-Medium.ttf",
        "~/.local/share/fonts/JetBrainsMonoNerdFont-Regular.ttf",
        "~/.local/share/fonts/JetBrainsMonoNLNerdFont-Regular.ttf",
        
        // JetBrains Mono Nerd Font - Linux system fonts
        "/usr/share/fonts/truetype/nerd-fonts/JetBrainsMonoNerdFont-Medium.ttf",
        "/usr/share/fonts/truetype/nerd-fonts/JetBrainsMonoNerdFont-Regular.ttf",
        "/usr/local/share/fonts/JetBrainsMonoNerdFont-Regular.ttf",
        
        // Regular JetBrains Mono (fallback)
        "~/Library/Fonts/JetBrains Mono Regular.ttf",
        "/Library/Fonts/JetBrains Mono Regular.ttf",
        "~/.local/share/fonts/JetBrains Mono Regular.ttf",
        
        // System default monospace fonts
        "/System/Library/Fonts/Menlo.ttf",   // macOS
        "/System/Library/Fonts/Courier.ttf", // macOS fallback
    ]
}

fn load_font_from_path(font_path: &str) -> Result<FontRef<'static>> {
    // Expand home directory if path starts with ~
    let expanded_path = if font_path.starts_with("~/") {
        if let Some(home_dir) = std::env::var_os("HOME") {
            let home_str = home_dir.to_string_lossy();
            font_path.replacen("~", &home_str, 1)
        } else {
            return Err(anyhow::anyhow!("HOME not set"));
        }
    } else {
        font_path.to_string()
    };
    
    let font_data = std::fs::read(&expanded_path)?;
    // Need to leak the data to get a 'static lifetime
    let static_data: &'static [u8] = Box::leak(font_data.into_boxed_slice());
    FontRef::try_from_slice(static_data)
        .map_err(|_| anyhow::anyhow!("Failed to parse font"))
}

fn get_font() -> Result<FontRef<'static>> {
    // Try to load fonts in order of preference, starting with user's preferred font
    let font_paths = get_font_paths();
    
    for font_path in &font_paths {
        if let Ok(font) = load_font_from_path(font_path) {
            println!("Using font: {}", font_path);
            return Ok(font);
        }
    }
    
    Err(anyhow::anyhow!(
        "No suitable monospace font found. For best results, install JetBrains Mono Nerd Font Medium:\n\
         - Download JetBrainsMonoNerdFontMono-Medium.ttf from: https://github.com/ryanoasis/nerd-fonts/releases\n\
         - Or use Homebrew: brew tap homebrew/cask-fonts && brew install font-jetbrains-mono-nerd-font\n\
         - Install to ~/Library/Fonts/ (macOS) or ~/.local/share/fonts/ (Linux)\n\
         \n\
         Fallback options:\n\
         - macOS: Monaco (should be pre-installed)\n\
         - Linux: sudo apt install fonts-dejavu fonts-liberation"
    ))
}

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
        let char_width = 8;  // Try even tighter spacing
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
    
    // Load font manager once for all frames
    let mut font_manager = FontManager::new()?;
    
    // Print fonts being used
    println!("Loaded {} fonts for better Unicode coverage", font_manager.fonts.len());
    
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
        
        let frame = render_terminal_to_image(&terminal_state, width, height, &mut font_manager)?;
        frames.push(frame);
        
        if frame_num % 30 == 0 {
            println!("Generated frame {} / {}", frame_num + 1, total_frames);
        }
    }
    
    Ok(frames)
}

fn calculate_font_baseline(font: &FontRef, font_size: f32) -> f32 {
    let scale = PxScale::from(font_size);
    
    // Get font metrics using ab_glyph unscaled API and apply scaling
    let units_per_em = font.units_per_em().unwrap_or(1000.0);
    let scale_factor = scale.y / units_per_em;
    
    let ascent = font.ascent_unscaled() * scale_factor;
    
    // Ensure baseline fits within our fixed 20px cells
    // If ascent is too large, cap it to leave room for descenders
    ascent.min(16.0)  // Leave ~4px for descenders in 20px cell
}

fn render_terminal_to_image(
    terminal_state: &TerminalState,
    width: u32,
    height: u32,
    font_manager: &mut FontManager,
) -> Result<RgbImage> {
    let mut image = ImageBuffer::new(width, height);
    let theme = terminal_state.get_theme();
    
    // Fill background
    let bg_color = Rgb([theme.background.0, theme.background.1, theme.background.2]);
    for pixel in image.pixels_mut() {
        *pixel = bg_color;
    }
    
    // Use fixed character cell dimensions to match terminal expectations
    let char_width = 8u32;  // Try even tighter spacing for terminal-like appearance
    let char_height = 20u32;
    let font_size = 14.0;  // Reduced from 16 to fit better in 20px cells
    let scale = PxScale::from(font_size);
    
    // Get the primary font for baseline calculation
    let primary_font = &font_manager.fonts[0];
    let baseline_offset = calculate_font_baseline(primary_font, font_size);
    
    let padding_x = 20;
    let padding_y = 20;
    
    let grid = terminal_state.get_grid();
    
    // Pass 1: Render all cell backgrounds first
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            // Use integer positioning to avoid gaps
            let pixel_x = padding_x + (x as u32 * char_width);
            let pixel_y = padding_y + (y as u32 * char_height);
            
            // Draw background color for this cell - ensure it fills the entire cell
            let bg_rect = Rect::at(pixel_x as i32, pixel_y as i32)
                .of_size(char_width, char_height);
            
            draw_filled_rect_mut(
                &mut image,
                bg_rect,
                Rgb([cell.bg_color.0, cell.bg_color.1, cell.bg_color.2]),
            );
        }
    }
    
    // Pass 2: Render all text on top of backgrounds
    for (y, row) in grid.iter().enumerate() {
        for (x, cell) in row.iter().enumerate() {
            // Use integer positioning to avoid gaps
            let pixel_x = padding_x + (x as u32 * char_width);
            let pixel_y = padding_y + (y as u32 * char_height);
            
            // Draw the character if it's not empty
            if cell.ch != ' ' && cell.ch != '\x00' {
                let text = cell.ch.to_string();
                let font_scale = if cell.bold { 
                    PxScale::from(font_size + 1.0) // Slightly larger for bold
                } else { 
                    scale 
                };
                
                // Get the best font for this character
                let font = font_manager.get_best_font_for_char(cell.ch);
                
                // Position text properly within the character cell
                // Center horizontally in the narrower 10px cell
                let text_x = pixel_x as i32;  // No offset needed for 10px cells
                let text_y = pixel_y as i32 + baseline_offset as i32;
                
                draw_text_mut(
                    &mut image,
                    Rgb([cell.fg_color.0, cell.fg_color.1, cell.fg_color.2]),
                    text_x,
                    text_y,
                    font_scale,
                    font,
                    &text,
                );
                
                // Add underline if needed
                if cell.underline {
                    let underline_rect = Rect::at(pixel_x as i32, (pixel_y + char_height - 2) as i32)
                        .of_size(char_width, 1);
                    
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