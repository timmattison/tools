use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "beta")]
#[command(about = "Beta - the superior terminal recorder. Because Betamax was always better than VHS.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Record {
        #[arg(short, long, help = "Output file for the recording")]
        output: Option<PathBuf>,
        
        #[arg(short, long, help = "Command to record (default: shell)")]
        command: Option<String>,
        
        #[arg(long, help = "Append to existing recording")]
        append: bool,
        
        #[arg(long, help = "Compress the recording with gzip")]
        compress: bool,
        
        #[arg(long, default_value = "ctrl-end", help = "Hotkey to stop recording (ctrl-end, f12, ctrl-], ctrl-\\\\)")]
        stop_hotkey: String,
    },
    Play {
        #[arg(help = "Recording file to play")]
        file: PathBuf,
        
        #[arg(short, long, default_value = "1.0", help = "Playback speed multiplier")]
        speed: f64,
        
        #[arg(long, help = "Start playback paused")]
        paused: bool,
    },
    Export {
        #[command(subcommand)]
        format: ExportFormat,
    },
}

#[derive(Subcommand)]
pub enum ExportFormat {
    Web {
        #[arg(help = "Recording file to export")]
        input: PathBuf,
        
        #[arg(short, long, help = "Output HTML file")]
        output: Option<PathBuf>,
        
        #[arg(long, default_value = "auto", help = "Theme (auto, dracula, monokai, solarized-dark, solarized-light)")]
        theme: String,
        
        #[arg(long, help = "Embed compressed data")]
        compress: bool,
    },
    Video {
        #[arg(help = "Recording file to export")]
        input: PathBuf,
        
        #[arg(short, long, help = "Output video file (MP4/GIF)")]
        output: Option<PathBuf>,
        
        #[arg(long, default_value = "60", help = "Frame rate (FPS)")]
        fps: u32,
        
        #[arg(long, help = "Resolution (WIDTHxHEIGHT)")]
        resolution: Option<String>,
        
        #[arg(long, default_value = "auto", help = "Theme for terminal rendering")]
        theme: String,
        
        #[arg(long, help = "Optimize for web delivery")]
        optimize_web: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub version: u32,
    pub width: u16,
    pub height: u16,
    pub timestamp: f64,
    pub duration: f64,
    pub command: String,
    pub title: String,
    pub env: std::collections::HashMap<String, String>,
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub time: f64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    #[serde(rename = "o")]
    Output,
    #[serde(rename = "i")]
    Input,
}

pub fn get_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

mod recorder;
mod player;
mod export;

#[cfg(test)]
mod test;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Record { output, command, append, compress, stop_hotkey } => {
            recorder::record(output, command, append, compress, stop_hotkey).await
        }
        Commands::Play { file, speed, paused } => {
            player::play(file, speed, paused).await
        }
        Commands::Export { format } => {
            export::handle_export(format).await
        }
    }
}