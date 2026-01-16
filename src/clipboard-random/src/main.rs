use anyhow::{Context, Result};
use arboard::Clipboard;
use base64::{engine::general_purpose, Engine};
use buildinfo::version_string;
use clap::{Parser, Subcommand};
use rand::Rng;

/// Generate random data and copy it to the clipboard/paste buffer
#[derive(Parser, Debug)]
#[clap(version = version_string!(), about)]
struct Args {
    #[clap(subcommand)]
    mode: Mode,
    
    /// Dry run mode - generate and display data without copying to clipboard
    #[clap(short, long)]
    dry_run: bool,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Generate random binary data
    Binary {
        /// Number of bytes of random data to generate
        #[clap(value_name = "BYTES")]
        bytes: usize,
        
        /// Output format for the random data
        #[clap(short, long, value_enum, default_value_t = OutputFormat::Hex)]
        format: OutputFormat,
    },
    /// Generate random text with diacritics (Zalgo text)
    Text {
        /// Number of characters of text to generate
        #[clap(value_name = "CHARS")]
        chars: usize,
        
        /// Probability (0.0-1.0) that each character will have diacritics
        #[clap(short, long, default_value_t = 0.5)]
        probability: f64,
        
        /// Minimum number of diacritics per character
        #[clap(long, default_value_t = 1)]
        min_diacritics: usize,
        
        /// Maximum number of diacritics per character
        #[clap(long, default_value_t = 3)]
        max_diacritics: usize,
        
        /// Minimum number of characters between spaces
        #[clap(long, default_value_t = 3)]
        min_word_length: usize,
        
        /// Maximum number of characters between spaces
        #[clap(long, default_value_t = 8)]
        max_word_length: usize,
        
        /// Use a preset configuration
        #[clap(long, value_enum)]
        preset: Option<TextPreset>,
    },
}

/// Output format for the random data
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq)]
enum OutputFormat {
    /// Hexadecimal representation (e.g., "a1b2c3")
    Hex,
    /// Base64 encoding
    Base64,
    /// Raw bytes as binary data
    Raw,
}

/// Text generation presets
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq)]
enum TextPreset {
    /// Mild diacritics effect
    Mild,
    /// Moderate diacritics effect
    Scary,
    /// Heavy diacritics effect
    Insane,
    /// Extreme diacritics effect (classic Zalgo)
    Zalgo,
    /// Apocalyptic diacritics effect
    Doom,
}

impl TextPreset {
    fn get_config(&self) -> TextConfig {
        match self {
            TextPreset::Mild => TextConfig {
                probability: 0.3,
                min_diacritics: 1,
                max_diacritics: 2,
                min_word_length: 4,
                max_word_length: 10,
            },
            TextPreset::Scary => TextConfig {
                probability: 0.6,
                min_diacritics: 1,
                max_diacritics: 4,
                min_word_length: 3,
                max_word_length: 8,
            },
            TextPreset::Insane => TextConfig {
                probability: 0.8,
                min_diacritics: 2,
                max_diacritics: 6,
                min_word_length: 2,
                max_word_length: 6,
            },
            TextPreset::Zalgo => TextConfig {
                probability: 0.9,
                min_diacritics: 3,
                max_diacritics: 8,
                min_word_length: 2,
                max_word_length: 5,
            },
            TextPreset::Doom => TextConfig {
                probability: 1.0,
                min_diacritics: 5,
                max_diacritics: 12,
                min_word_length: 1,
                max_word_length: 4,
            },
        }
    }
}

#[derive(Debug)]
struct TextConfig {
    probability: f64,
    min_diacritics: usize,
    max_diacritics: usize,
    min_word_length: usize,
    max_word_length: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    
    match args.mode {
        Mode::Binary { bytes, format } => {
            generate_binary_data(bytes, format, args.dry_run)
        }
        Mode::Text { 
            chars, 
            probability, 
            min_diacritics, 
            max_diacritics, 
            min_word_length, 
            max_word_length,
            preset 
        } => {
            let config = if let Some(preset) = preset {
                preset.get_config()
            } else {
                TextConfig {
                    probability,
                    min_diacritics,
                    max_diacritics,
                    min_word_length,
                    max_word_length,
                }
            };
            generate_text_data(chars, config, args.dry_run)
        }
    }
}

fn generate_binary_data(bytes: usize, format: OutputFormat, dry_run: bool) -> Result<()> {
    // Validate input
    if bytes == 0 {
        anyhow::bail!("Number of bytes must be greater than 0");
    }
    
    // Generate random data
    let mut rng = rand::rng();
    let random_bytes: Vec<u8> = (0..bytes).map(|_| rng.random()).collect();
    
    // Handle raw binary data differently
    if format == OutputFormat::Raw {
        // Copy raw binary data to clipboard
        if dry_run {
            println!("{:?}", random_bytes);
            return Ok(());
        } else {
            copy_binary_to_clipboard(&random_bytes)?;
            println!("Generated {} bytes of raw binary data and copied to clipboard", bytes);
        }
        return Ok(());
    }
    
    // Format the data according to the specified format
    let formatted_data = match format {
        OutputFormat::Hex => hex_encode(&random_bytes),
        OutputFormat::Base64 => general_purpose::STANDARD.encode(&random_bytes),
        OutputFormat::Raw => unreachable!(), // Handled above
    };
    
    // Copy to clipboard (unless in dry run mode)
    if dry_run {
        println!("{}", formatted_data);
        return Ok(());
    } else {
        let mut clipboard = Clipboard::new()
            .context("Failed to access clipboard. Make sure you're running in a graphical environment.")?;
        
        clipboard.set_text(formatted_data.clone())
            .context("Failed to copy data to clipboard")?;
        
        println!("Generated {} bytes of random data and copied to clipboard", bytes);
    }
    println!("Format: {:?}", format);
    println!("Data length: {} characters", formatted_data.len());
    
    // Show a preview of the data (first 50 characters)
    let preview = if formatted_data.len() > 50 {
        format!("{}...", &formatted_data[..50])
    } else {
        formatted_data
    };
    println!("Preview: {}", preview);
    
    Ok(())
}

fn generate_text_data(chars: usize, config: TextConfig, dry_run: bool) -> Result<()> {
    // Validate input
    if chars == 0 {
        anyhow::bail!("Number of characters must be greater than 0");
    }
    
    if config.probability < 0.0 || config.probability > 1.0 {
        anyhow::bail!("Probability must be between 0.0 and 1.0");
    }
    
    if config.min_diacritics > config.max_diacritics {
        anyhow::bail!("Minimum diacritics cannot be greater than maximum diacritics");
    }
    
    if config.min_word_length > config.max_word_length {
        anyhow::bail!("Minimum word length cannot be greater than maximum word length");
    }
    
    // Generate text with diacritics
    let text = generate_zalgo_text(chars, &config)?;
    
    // Copy to clipboard (unless in dry run mode)
    if dry_run {
        println!("{}", text);
        return Ok(());
    } else {
        let mut clipboard = Clipboard::new()
            .context("Failed to access clipboard. Make sure you're running in a graphical environment.")?;
        
        clipboard.set_text(text.clone())
            .context("Failed to copy text to clipboard")?;
        
        println!("Generated {} characters of text with diacritics and copied to clipboard", chars);
    }
    
    println!("Text length: {} characters", text.len());
    println!("Config: probability={:.2}, diacritics={}-{}, word_length={}-{}", 
             config.probability, config.min_diacritics, config.max_diacritics,
             config.min_word_length, config.max_word_length);
    
    // Show a preview of the text (first 100 characters)
    let preview = if text.chars().count() > 100 {
        format!("{}...", text.chars().take(100).collect::<String>())
    } else {
        text
    };
    println!("Preview: {}", preview);
    
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn copy_binary_to_clipboard(data: &[u8]) -> Result<()> {
    // For raw binary data, we need to use the image functionality of the clipboard
    // or handle it as bytes. Since arboard primarily handles text and images,
    // we'll encode as latin-1 which preserves all byte values
    let text = data.iter().map(|&b| char::from(b)).collect::<String>();
    
    let mut clipboard = Clipboard::new()
        .context("Failed to access clipboard. Make sure you're running in a graphical environment.")?;
    
    clipboard.set_text(text)
        .context("Failed to copy binary data to clipboard")?;
    
    Ok(())
}

fn generate_zalgo_text(chars: usize, config: &TextConfig) -> Result<String> {
    let mut rng = rand::rng();
    let mut result = String::new();
    let mut chars_added = 0;
    
    // Base characters for text generation (ASCII letters)
    let base_chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ".chars().collect();
    
    // Diacritical marks for Zalgo text
    let combining_marks = get_combining_marks();
    
    while chars_added < chars {
        // Generate a word
        let word_length = rng.random_range(config.min_word_length..=config.max_word_length);
        let word_length = std::cmp::min(word_length, chars - chars_added);
        
        for _ in 0..word_length {
            if chars_added >= chars {
                break;
            }
            
            // Add a base character
            let base_char = base_chars[rng.random_range(0..base_chars.len())];
            result.push(base_char);
            chars_added += 1;
            
            // Potentially add diacritics
            if rng.random::<f64>() < config.probability {
                let num_diacritics = rng.random_range(config.min_diacritics..=config.max_diacritics);
                for _ in 0..num_diacritics {
                    let mark = combining_marks[rng.random_range(0..combining_marks.len())];
                    result.push(mark);
                }
            }
        }
        
        // Add space if we haven't reached the character limit
        if chars_added < chars {
            result.push(' ');
            chars_added += 1;
        }
    }
    
    Ok(result)
}

fn get_combining_marks() -> Vec<char> {
    // Unicode combining diacritical marks for Zalgo text
    vec![
        // Combining diacritical marks (above)
        '\u{0300}', // Combining grave accent
        '\u{0301}', // Combining acute accent
        '\u{0302}', // Combining circumflex accent
        '\u{0303}', // Combining tilde
        '\u{0304}', // Combining macron
        '\u{0305}', // Combining overline
        '\u{0306}', // Combining breve
        '\u{0307}', // Combining dot above
        '\u{0308}', // Combining diaeresis
        '\u{0309}', // Combining hook above
        '\u{030A}', // Combining ring above
        '\u{030B}', // Combining double acute accent
        '\u{030C}', // Combining caron
        '\u{030D}', // Combining vertical line above
        '\u{030E}', // Combining double vertical line above
        '\u{030F}', // Combining double grave accent
        '\u{0310}', // Combining candrabindu
        '\u{0311}', // Combining inverted breve
        '\u{0312}', // Combining turned comma above
        '\u{0313}', // Combining comma above
        '\u{0314}', // Combining reversed comma above
        '\u{0315}', // Combining comma above right
        '\u{0316}', // Combining grave accent below
        '\u{0317}', // Combining acute accent below
        '\u{0318}', // Combining left tack below
        '\u{0319}', // Combining right tack below
        '\u{031A}', // Combining left angle above
        '\u{031B}', // Combining horn
        '\u{031C}', // Combining left half ring below
        '\u{031D}', // Combining up tack below
        '\u{031E}', // Combining down tack below
        '\u{031F}', // Combining plus sign below
        '\u{0320}', // Combining minus sign below
        '\u{0321}', // Combining palatalized hook below
        '\u{0322}', // Combining retroflex hook below
        '\u{0323}', // Combining dot below
        '\u{0324}', // Combining diaeresis below
        '\u{0325}', // Combining ring below
        '\u{0326}', // Combining comma below
        '\u{0327}', // Combining cedilla
        '\u{0328}', // Combining ogonek
        '\u{0329}', // Combining vertical line below
        '\u{032A}', // Combining bridge below
        '\u{032B}', // Combining inverted double arch below
        '\u{032C}', // Combining caron below
        '\u{032D}', // Combining circumflex accent below
        '\u{032E}', // Combining breve below
        '\u{032F}', // Combining inverted breve below
        '\u{0330}', // Combining tilde below
        '\u{0331}', // Combining macron below
        '\u{0332}', // Combining low line
        '\u{0333}', // Combining double low line
        '\u{0334}', // Combining tilde overlay
        '\u{0335}', // Combining short stroke overlay
        '\u{0336}', // Combining long stroke overlay
        '\u{0337}', // Combining short solidus overlay
        '\u{0338}', // Combining long solidus overlay
        '\u{0339}', // Combining right half ring below
        '\u{033A}', // Combining inverted bridge below
        '\u{033B}', // Combining square below
        '\u{033C}', // Combining seagull below
        '\u{033D}', // Combining x above
        '\u{033E}', // Combining vertical tilde
        '\u{033F}', // Combining double overline
        '\u{0340}', // Combining grave tone mark
        '\u{0341}', // Combining acute tone mark
        '\u{0342}', // Combining perispomeni
        '\u{0343}', // Combining koronis
        '\u{0344}', // Combining dialytika tonos
        '\u{0345}', // Combining iota subscript
        '\u{0346}', // Combining bridge above
        '\u{0347}', // Combining equals sign below
        '\u{0348}', // Combining double vertical line below
        '\u{0349}', // Combining left angle below
        '\u{034A}', // Combining not tilde above
        '\u{034B}', // Combining homothetic above
        '\u{034C}', // Combining almost equal to above
        '\u{034D}', // Combining left right arrow below
        '\u{034E}', // Combining upwards arrow below
        '\u{034F}', // Combining grapheme joiner
        '\u{0350}', // Combining right arrowhead above
        '\u{0351}', // Combining left half ring above
        '\u{0352}', // Combining fermata
        '\u{0353}', // Combining x below
        '\u{0354}', // Combining left arrowhead below
        '\u{0355}', // Combining right arrowhead below
        '\u{0356}', // Combining right arrowhead and up arrowhead below
        '\u{0357}', // Combining right half ring above
        '\u{0358}', // Combining dot above right
        '\u{0359}', // Combining asterisk below
        '\u{035A}', // Combining double ring below
        '\u{035B}', // Combining zigzag above
        '\u{035C}', // Combining double breve below
        '\u{035D}', // Combining double breve
        '\u{035E}', // Combining double macron
        '\u{035F}', // Combining double macron below
        '\u{0360}', // Combining double tilde
        '\u{0361}', // Combining double inverted breve
        '\u{0362}', // Combining double rightwards arrow below
    ]
}