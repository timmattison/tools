use anyhow::{Context, Result};
use base64::prelude::*;
use clap::Parser;
use image::{ImageFormat, DynamicImage};
use std::io::{self, BufReader, Read, Write};
use std::path::PathBuf;

/// Fast terminal image display utility for iTerm2
#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Image file to display
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

    /// Image quality for JPEG output (1-100, default: 90)
    #[clap(short, long, default_value = "90")]
    quality: u8,

    /// Output format (png, jpeg, auto)
    #[clap(short, long, default_value = "auto")]
    format: String,
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

    if args.quality == 0 || args.quality > 100 {
        eprintln!("Error: Quality must be between 1 and 100");
        std::process::exit(1);
    }

    if !["auto", "png", "jpeg"].contains(&args.format.as_str()) {
        eprintln!("Error: Format must be 'auto', 'png', or 'jpeg'");
        std::process::exit(1);
    }

    // Show tmux warning if detected
    if std::env::var("TMUX").is_ok() {
        eprintln!("Warning: tmux detected. This utility does not work in tmux. Please run it directly in your terminal.");
    }

    if args.stdin {
        display_image_from_stdin(&args)?;
    } else if let Some(ref file_path) = args.file {
        display_image_from_file(&file_path, &args)?;
    }

    Ok(())
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
    reader.read_to_end(&mut buffer)
        .context("Failed to read image data from stdin")?;

    let img = image::load_from_memory(&buffer)
        .context("Failed to decode image from stdin")?;

    display_image(img, args)
}

fn display_image(mut img: DynamicImage, args: &Args) -> Result<()> {
    // Resize image if dimensions are specified
    let needs_resize = args.width.is_some() || args.height.is_some();
    
    if needs_resize {
        if let (Some(width), Some(height)) = (args.width, args.height) {
            img = if args.preserve_aspect {
                img.resize(width, height, image::imageops::FilterType::Triangle)
            } else {
                img.resize_exact(width, height, image::imageops::FilterType::Triangle)
            };
        } else if let Some(width) = args.width {
            let height = (img.height() * width) / img.width();
            img = img.resize(width, height, image::imageops::FilterType::Triangle);
        } else if let Some(height) = args.height {
            let width = (img.width() * height) / img.height();
            img = img.resize(width, height, image::imageops::FilterType::Triangle);
        }
    }

    // Convert image to the specified format for encoding
    let mut encoded_data = Vec::with_capacity(img.width() as usize * img.height() as usize * 4);
    
    let output_format = match args.format.as_str() {
        "png" => ImageFormat::Png,
        "jpeg" => ImageFormat::Jpeg,
        "auto" => {
            // Choose format based on whether image has transparency
            if img.color().has_alpha() {
                ImageFormat::Png
            } else {
                ImageFormat::Jpeg
            }
        }
        _ => ImageFormat::Png, // Default fallback
    };

    match output_format {
        ImageFormat::Jpeg => {
            // For JPEG, we need to use a specific encoder to set quality
            let rgb_img = img.to_rgb8();
            let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
                &mut encoded_data, 
                args.quality
            );
            encoder.encode_image(&rgb_img)
                .context("Failed to encode image as JPEG")?;
        }
        _ => {
            img.write_to(&mut std::io::Cursor::new(&mut encoded_data), output_format)
                .context("Failed to encode image")?;
        }
    }

    // Base64 encode the image data with pre-allocated capacity
    let encoded = BASE64_STANDARD.encode(&encoded_data);

    // Output iTerm2 inline image escape sequence
    print_iterm2_image(&encoded, img.width(), img.height(), args.no_newline)?;

    Ok(())
}

fn print_iterm2_image(base64_data: &str, width: u32, height: u32, no_newline: bool) -> Result<()> {
    let mut stdout = io::stdout().lock();
    
    // iTerm2 inline image protocol
    // ESC ] 1337 ; File = [arguments] : base64_data BEL
    write!(stdout, "\x1b]1337;File=inline=1")?;
    
    // Add width and height if we want to control display size
    write!(stdout, ";width={}px;height={}px", width, height)?;
    
    if no_newline {
        write!(stdout, ":{}\x07", base64_data)?;
    } else {
        write!(stdout, ":{}\x07\n", base64_data)?;
    }
    
    stdout.flush().context("Failed to flush output")?;
    Ok(())
}
