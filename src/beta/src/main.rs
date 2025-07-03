use anyhow::Result;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser)]
#[command(name = "beta")]
#[command(about = "Terminal session recorder and player", long_about = None)]
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
    },
    Play {
        #[arg(help = "Recording file to play")]
        file: PathBuf,
        
        #[arg(short, long, default_value = "1.0", help = "Playback speed multiplier")]
        speed: f64,
        
        #[arg(long, help = "Start playback paused")]
        paused: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Recording {
    version: u32,
    width: u16,
    height: u16,
    timestamp: f64,
    duration: f64,
    command: String,
    title: String,
    env: std::collections::HashMap<String, String>,
    events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event {
    time: f64,
    #[serde(rename = "type")]
    event_type: EventType,
    data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum EventType {
    #[serde(rename = "o")]
    Output,
    #[serde(rename = "i")]
    Input,
}

fn get_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

mod recorder;
mod player;

#[cfg(test)]
mod test;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Record { output, command, append, compress } => {
            recorder::record(output, command, append, compress).await
        }
        Commands::Play { file, speed, paused } => {
            player::play(file, speed, paused).await
        }
    }
}