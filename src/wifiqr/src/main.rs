use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use image::{DynamicImage, GenericImage, GenericImageView, Rgba};
use qrcode::{QrCode, EcLevel};

#[derive(Parser)]
#[command(name = "wifiqr")]
#[command(version = version_string!())]
#[command(about = "Generate a QR code for WiFi network access")]
#[command(long_about = None)]
struct Cli {
    #[arg(short, long, required = true, help = "WiFi network name (SSID)")]
    ssid: String,
    
    #[arg(short, long, required = true, help = "WiFi network password")]
    password: String,
    
    #[arg(short, long, default_value = "1024", help = "Resolution of the QR code image (width and height in pixels)")]
    resolution: u32,
    
    #[arg(short, long, help = "Path to an image file to use as a logo in the center of the QR code")]
    logo: Option<String>,
    
    #[arg(long, default_value = "10.0", help = "Size of the logo as a percentage of the QR code (1-100)")]
    logo_size: f64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    
    // Generate the QR code
    generate_wifi_qr_code(&cli.ssid, &cli.password, cli.resolution, cli.logo.as_deref(), cli.logo_size)?;
    
    println!("QR code generated successfully: {}.png", cli.ssid);
    Ok(())
}

fn generate_wifi_qr_code(ssid: &str, password: &str, resolution: u32, logo_file: Option<&str>, logo_size: f64) -> Result<()> {
    // Format the WiFi connection string
    let wifi_string = format!("WIFI:S:{};T:WPA;P:{};;", escape_special_chars(ssid), escape_special_chars(password));
    
    // Create QR code with high error correction
    let code = QrCode::with_error_correction_level(&wifi_string, EcLevel::H)
        .context("Failed to create QR code")?;
    
    // Render to image - start with a reasonable base size
    let base_size = (21 + (code.width() - 21) * 4) as u32; // Scale based on QR code version
    let img = code.render::<Rgba<u8>>()
        .quiet_zone(false)
        .module_dimensions(base_size, base_size)
        .build();
    
    // Convert to DynamicImage for easier manipulation
    let mut qr_image = DynamicImage::ImageRgba8(img);
    
    // Add logo if specified
    if let Some(logo_path) = logo_file {
        qr_image = add_logo_to_qr(qr_image, logo_path, logo_size)?;
    }
    
    // Resize to requested resolution
    let final_image = resize_image(qr_image, resolution);
    
    // Save the image
    let output_file = format!("{}.png", ssid);
    final_image.save(&output_file)
        .context("Failed to save QR code image")?;
    
    Ok(())
}

fn escape_special_chars(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(':', "\\:")
}

fn add_logo_to_qr(qr_image: DynamicImage, logo_path: &str, logo_size_percent: f64) -> Result<DynamicImage> {
    // Load the logo image
    let logo = image::open(logo_path)
        .context("Failed to open logo image")?;
    
    // Get logo dimensions before moving it
    let logo_dimensions = logo.dimensions();
    
    // Get QR code dimensions
    let (qr_width, qr_height) = qr_image.dimensions();
    
    // Calculate logo size (capped at 100%)
    let logo_size_percent = logo_size_percent.min(100.0);
    
    // Maximum logo size is 1/5 of QR code size
    let max_logo_size = (qr_width.min(qr_height) / 5) as u32;
    let desired_logo_size = (max_logo_size as f64 * logo_size_percent / 100.0) as u32;
    let desired_logo_size = desired_logo_size.max(1);
    
    // Resize logo to square
    let logo_resized = resize_image(logo, desired_logo_size);
    
    // Create a new image with the QR code
    let mut combined = qr_image.clone();
    
    // Calculate position to center the logo
    let x_pos = (qr_width - desired_logo_size) / 2;
    let y_pos = (qr_height - desired_logo_size) / 2;
    
    // Create white background for logo (to ensure it's visible)
    let padding = 4;
    let bg_size = desired_logo_size + padding * 2;
    let bg_x = x_pos.saturating_sub(padding);
    let bg_y = y_pos.saturating_sub(padding);
    
    // Draw white background
    for y in bg_y..bg_y.saturating_add(bg_size).min(qr_height) {
        for x in bg_x..bg_x.saturating_add(bg_size).min(qr_width) {
            combined.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        }
    }
    
    // Overlay the logo
    image::imageops::overlay(&mut combined, &logo_resized, x_pos.into(), y_pos.into());
    
    println!("Logo dimensions: original={:?}, max_size={}, desired_size={}", 
             logo_dimensions, max_logo_size, desired_logo_size);
    
    Ok(combined)
}

fn resize_image(img: DynamicImage, target_size: u32) -> DynamicImage {
    // Use nearest neighbor for sharp edges (important for QR codes)
    img.resize_exact(target_size, target_size, image::imageops::FilterType::Nearest)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_escape_special_chars() {
        assert_eq!(escape_special_chars("test"), "test");
        assert_eq!(escape_special_chars("test;123"), "test\\;123");
        assert_eq!(escape_special_chars("test:123"), "test\\:123");
        assert_eq!(escape_special_chars("test\\123"), "test\\\\123");
        assert_eq!(escape_special_chars("a;b:c\\d"), "a\\;b\\:c\\\\d");
    }
    
    #[test]
    fn test_wifi_string_format() {
        let ssid = "MyNetwork";
        let password = "pass;word";
        let expected = "WIFI:S:MyNetwork;T:WPA;P:pass\\;word;;";
        let actual = format!("WIFI:S:{};T:WPA;P:{};;", 
                           escape_special_chars(ssid), 
                           escape_special_chars(password));
        assert_eq!(actual, expected);
    }
}