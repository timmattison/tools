use anyhow::Result;
use clap::Parser;
use clipboardmon::{monitor_clipboard, Transformer, DEFAULT_POLL_INTERVAL};
use log::error;
use std::error::Error;
use std::sync::{Arc, Mutex};

const WARNING_PREFIX: &str = "DANGEROUS PASTE CONTENT AHEAD! ";

#[derive(Parser)]
#[command(name = "safeboard")]
#[command(about = "Monitor clipboard for dangerous Unicode characters")]
#[command(long_about = "Detects invisible Unicode characters and unsafe code points that could be used in attacks")]
struct Cli {
    #[arg(long, help = "Play a sound when dangerous content is detected")]
    audible: bool,
    
    #[arg(long, help = "Modify clipboard by prepending a warning message")]
    modify: bool,
}

struct SafeboardTransformer {
    config: SafeboardConfig,
    last_warned_content: Arc<Mutex<Option<String>>>,
}

struct SafeboardConfig {
    audible: bool,
    modify: bool,
}

impl SafeboardTransformer {
    fn new(config: SafeboardConfig) -> Self {
        Self {
            config,
            last_warned_content: Arc::new(Mutex::new(None)),
        }
    }
    
    fn contains_dangerous_characters(content: &str) -> Vec<DangerousChar> {
        let mut dangerous = Vec::new();
        
        for (index, ch) in content.char_indices() {
            let danger = match ch {
                // Zero-width characters
                '\u{200B}' => Some(DangerousChar::new(ch, index, "Zero-width space")),
                '\u{200C}' => Some(DangerousChar::new(ch, index, "Zero-width non-joiner")),
                '\u{200D}' => Some(DangerousChar::new(ch, index, "Zero-width joiner")),
                '\u{FEFF}' => Some(DangerousChar::new(ch, index, "Zero-width no-break space")),
                
                // Directional override characters
                '\u{202A}' => Some(DangerousChar::new(ch, index, "Left-to-right embedding")),
                '\u{202B}' => Some(DangerousChar::new(ch, index, "Right-to-left embedding")),
                '\u{202C}' => Some(DangerousChar::new(ch, index, "Pop directional formatting")),
                '\u{202D}' => Some(DangerousChar::new(ch, index, "Left-to-right override")),
                '\u{202E}' => Some(DangerousChar::new(ch, index, "Right-to-left override")),
                
                // Private use area (U+E000 - U+F8FF)
                '\u{E000}'..='\u{F8FF}' => Some(DangerousChar::new(ch, index, "Private use area character")),
                
                // Other invisible/confusing characters
                '\u{2060}' => Some(DangerousChar::new(ch, index, "Word joiner")),
                '\u{2061}' => Some(DangerousChar::new(ch, index, "Function application")),
                '\u{2062}' => Some(DangerousChar::new(ch, index, "Invisible times")),
                '\u{2063}' => Some(DangerousChar::new(ch, index, "Invisible separator")),
                '\u{2064}' => Some(DangerousChar::new(ch, index, "Invisible plus")),
                
                _ => None,
            };
            
            if let Some(d) = danger {
                dangerous.push(d);
            }
        }
        
        dangerous
    }
    
    fn play_alert_sound() {
        // Try to play a system beep sound
        if let Err(e) = Self::play_beep() {
            error!("Failed to play alert sound: {}", e);
        }
    }
    
    fn play_beep() -> Result<()> {
        // Use rodio to play a simple beep
        use rodio::{OutputStream, Sink};
        use rodio::source::{SineWave, Source};
        use std::time::Duration;
        
        let (_stream, stream_handle) = OutputStream::try_default()?;
        let sink = Sink::try_new(&stream_handle)?;
        
        // Create an 800Hz beep for 200ms
        let source = SineWave::new(800.0)
            .take_duration(Duration::from_millis(200))
            .amplify(0.5);
        
        sink.append(source);
        sink.sleep_until_end();
        
        Ok(())
    }
}

struct DangerousChar {
    character: char,
    position: usize,
    description: &'static str,
}

impl DangerousChar {
    fn new(character: char, position: usize, description: &'static str) -> Self {
        Self { character, position, description }
    }
}

impl Transformer for SafeboardTransformer {
    fn is_relevant(&self, content: &str) -> bool {
        // Check if this is content we already warned about
        if let Ok(last_warned) = self.last_warned_content.lock() {
            if let Some(ref warned) = *last_warned {
                if content == warned {
                    return false; // Skip our own warned content
                }
            }
        }
        
        // Check if content contains dangerous characters
        !Self::contains_dangerous_characters(content).is_empty()
    }
    
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
        let dangerous_chars = Self::contains_dangerous_characters(content);
        
        // Print warnings for each dangerous character found
        println!("⚠️  DANGEROUS CONTENT DETECTED!");
        println!("Found {} dangerous character(s):", dangerous_chars.len());
        for danger in &dangerous_chars {
            println!(
                "  - Position {}: {} (U+{:04X})",
                danger.position,
                danger.description,
                danger.character as u32
            );
        }
        
        // Play sound if requested
        if self.config.audible {
            Self::play_alert_sound();
        }
        
        // Modify clipboard if requested
        if self.config.modify {
            let warned_content = if content.starts_with(WARNING_PREFIX) {
                // Content already has a warning (possible bypass attempt)
                // Add another warning to make it clear
                format!("{}{}", WARNING_PREFIX, content)
            } else {
                format!("{}{}", WARNING_PREFIX, content)
            };
            
            // Store what we're putting in the clipboard
            if let Ok(mut last_warned) = self.last_warned_content.lock() {
                *last_warned = Some(warned_content.clone());
            }
            
            Ok(warned_content)
        } else {
            // Don't modify, just report
            println!("Clipboard not modified (use --modify flag to add warning prefix)");
            Err("Dangerous content detected but not modified".into())
        }
    }
    
    fn waiting_message(&self) -> &str {
        "Monitoring clipboard for dangerous Unicode characters"
    }
    
    fn success_message(&self) -> &str {
        "⚠️  DANGEROUS CONTENT DETECTED AND HANDLED!"
    }
}

fn main() -> Result<()> {
    env_logger::init();
    
    let cli = Cli::parse();
    
    let config = SafeboardConfig {
        audible: cli.audible,
        modify: cli.modify,
    };
    
    println!("Starting safeboard with options: audible={}, modify={}", 
         config.audible, config.modify);
    println!("Monitoring clipboard for dangerous Unicode characters...");
    
    let transformer = SafeboardTransformer::new(config);
    monitor_clipboard(transformer, DEFAULT_POLL_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_dangerous_character_detection() {
        // Test zero-width space
        let content = "Hello\u{200B}World";
        let dangerous = SafeboardTransformer::contains_dangerous_characters(content);
        assert_eq!(dangerous.len(), 1);
        assert_eq!(dangerous[0].description, "Zero-width space");
        
        // Test RTL override
        let content = "Hello\u{202E}World";
        let dangerous = SafeboardTransformer::contains_dangerous_characters(content);
        assert_eq!(dangerous.len(), 1);
        assert_eq!(dangerous[0].description, "Right-to-left override");
        
        // Test private use area
        let content = "Hello\u{E000}World";
        let dangerous = SafeboardTransformer::contains_dangerous_characters(content);
        assert_eq!(dangerous.len(), 1);
        assert_eq!(dangerous[0].description, "Private use area character");
        
        // Test safe content
        let content = "Hello World";
        let dangerous = SafeboardTransformer::contains_dangerous_characters(content);
        assert_eq!(dangerous.len(), 0);
    }
    
    #[test]
    fn test_multiple_dangerous_characters() {
        let content = "A\u{200B}B\u{202E}C\u{E000}D";
        let dangerous = SafeboardTransformer::contains_dangerous_characters(content);
        assert_eq!(dangerous.len(), 3);
    }
}