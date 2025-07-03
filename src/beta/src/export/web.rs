use anyhow::{Context, Result};
use minijinja::{Environment, context};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use crate::Recording;

const HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{{ title }}</title>
    <style>
        body {
            margin: 0;
            padding: 20px;
            background-color: #1e1e1e;
            color: #ffffff;
            font-family: "Monaco", "Menlo", "Ubuntu Mono", monospace;
        }
        .container {
            max-width: 1200px;
            margin: 0 auto;
        }
        .header {
            text-align: center;
            margin-bottom: 20px;
        }
        .terminal-container {
            background-color: {{ theme.background }};
            border-radius: 8px;
            padding: 20px;
            box-shadow: 0 4px 8px rgba(0, 0, 0, 0.3);
            position: relative;
        }
        .terminal {
            font-family: "Monaco", "Menlo", "Ubuntu Mono", monospace;
            font-size: 14px;
            line-height: 1.2;
            color: {{ theme.foreground }};
            background-color: {{ theme.background }};
            white-space: pre;
            overflow-x: auto;
            min-height: 400px;
            position: relative;
        }
        .controls {
            margin-top: 20px;
            text-align: center;
        }
        .controls button {
            background-color: #4a4a4a;
            color: white;
            border: none;
            padding: 10px 20px;
            margin: 0 5px;
            border-radius: 5px;
            cursor: pointer;
            font-family: inherit;
        }
        .controls button:hover {
            background-color: #5a5a5a;
        }
        .controls button:disabled {
            background-color: #2a2a2a;
            cursor: not-allowed;
        }
        .progress-bar {
            width: 100%;
            height: 4px;
            background-color: #333;
            border-radius: 2px;
            margin: 10px 0;
            overflow: hidden;
        }
        .progress-fill {
            height: 100%;
            background-color: #007acc;
            width: 0%;
            transition: width 0.1s ease;
        }
        .time-display {
            text-align: center;
            margin: 10px 0;
            font-size: 12px;
            color: #888;
        }
        .speed-control {
            margin: 10px 0;
            text-align: center;
        }
        .speed-control select {
            background-color: #4a4a4a;
            color: white;
            border: 1px solid #666;
            padding: 5px 10px;
            border-radius: 3px;
            font-family: inherit;
        }
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>{{ title }}</h1>
            <p>Duration: {{ duration }}s | {{ events_count }} events</p>
        </div>
        
        <div class="terminal-container">
            <div class="terminal" id="terminal"></div>
            
            <div class="progress-bar">
                <div class="progress-fill" id="progress"></div>
            </div>
            
            <div class="time-display">
                <span id="current-time">0:00</span> / <span id="total-time">{{ duration_formatted }}</span>
            </div>
            
            <div class="controls">
                <button id="play-pause">▶️ Play</button>
                <button id="restart">⏮️ Restart</button>
                <button id="step-back">⏪ -5s</button>
                <button id="step-forward">⏩ +5s</button>
            </div>
            
            <div class="speed-control">
                Speed: 
                <select id="speed">
                    <option value="0.25">0.25x</option>
                    <option value="0.5">0.5x</option>
                    <option value="1" selected>1x</option>
                    <option value="1.5">1.5x</option>
                    <option value="2">2x</option>
                    <option value="3">3x</option>
                </select>
            </div>
        </div>
    </div>

    <script>
        class TerminalPlayer {
            constructor(recording) {
                this.recording = recording;
                this.events = recording.events;
                this.currentEventIndex = 0;
                this.isPlaying = false;
                this.playbackSpeed = 1.0;
                this.startTime = null;
                this.pausedAt = 0;
                
                this.terminal = document.getElementById('terminal');
                this.playPauseBtn = document.getElementById('play-pause');
                this.progressBar = document.getElementById('progress');
                this.currentTimeDisplay = document.getElementById('current-time');
                this.speedSelect = document.getElementById('speed');
                
                this.bindEvents();
                this.updateDisplay();
            }
            
            bindEvents() {
                this.playPauseBtn.addEventListener('click', () => this.togglePlayPause());
                document.getElementById('restart').addEventListener('click', () => this.restart());
                document.getElementById('step-back').addEventListener('click', () => this.stepBack());
                document.getElementById('step-forward').addEventListener('click', () => this.stepForward());
                this.speedSelect.addEventListener('change', (e) => {
                    this.playbackSpeed = parseFloat(e.target.value);
                });
                
                document.addEventListener('keydown', (e) => {
                    if (e.code === 'Space') {
                        e.preventDefault();
                        this.togglePlayPause();
                    } else if (e.code === 'ArrowLeft') {
                        e.preventDefault();
                        this.stepBack();
                    } else if (e.code === 'ArrowRight') {
                        e.preventDefault();
                        this.stepForward();
                    }
                });
            }
            
            togglePlayPause() {
                if (this.isPlaying) {
                    this.pause();
                } else {
                    this.play();
                }
            }
            
            play() {
                this.isPlaying = true;
                this.playPauseBtn.textContent = '⏸️ Pause';
                this.startTime = Date.now() - this.pausedAt * 1000;
                this.animate();
            }
            
            pause() {
                this.isPlaying = false;
                this.playPauseBtn.textContent = '▶️ Play';
                if (this.currentEventIndex < this.events.length) {
                    this.pausedAt = this.events[this.currentEventIndex].time;
                }
            }
            
            restart() {
                this.pause();
                this.currentEventIndex = 0;
                this.pausedAt = 0;
                this.terminal.textContent = '';
                this.updateDisplay();
            }
            
            stepBack() {
                const targetTime = Math.max(0, this.getCurrentTime() - 5);
                this.seekTo(targetTime);
            }
            
            stepForward() {
                const targetTime = Math.min(this.recording.duration, this.getCurrentTime() + 5);
                this.seekTo(targetTime);
            }
            
            seekTo(time) {
                const wasPlaying = this.isPlaying;
                this.pause();
                
                this.currentEventIndex = 0;
                this.terminal.textContent = '';
                
                for (let i = 0; i < this.events.length; i++) {
                    if (this.events[i].time <= time && this.events[i].type === 'o') {
                        this.terminal.textContent += this.events[i].data;
                        this.currentEventIndex = i + 1;
                    }
                }
                
                this.pausedAt = time;
                this.updateDisplay();
                
                if (wasPlaying) {
                    this.play();
                }
            }
            
            getCurrentTime() {
                if (this.isPlaying && this.startTime) {
                    return (Date.now() - this.startTime) / 1000 * this.playbackSpeed;
                }
                return this.pausedAt;
            }
            
            animate() {
                if (!this.isPlaying) return;
                
                const currentTime = this.getCurrentTime();
                
                while (this.currentEventIndex < this.events.length && 
                       this.events[this.currentEventIndex].time <= currentTime) {
                    const event = this.events[this.currentEventIndex];
                    if (event.type === 'o') {
                        this.terminal.textContent += event.data;
                    }
                    this.currentEventIndex++;
                }
                
                this.updateDisplay();
                
                if (this.currentEventIndex >= this.events.length) {
                    this.pause();
                    return;
                }
                
                requestAnimationFrame(() => this.animate());
            }
            
            updateDisplay() {
                const currentTime = this.getCurrentTime();
                const progress = (currentTime / this.recording.duration) * 100;
                this.progressBar.style.width = `${Math.min(100, progress)}%`;
                
                const minutes = Math.floor(currentTime / 60);
                const seconds = Math.floor(currentTime % 60);
                this.currentTimeDisplay.textContent = `${minutes}:${seconds.toString().padStart(2, '0')}`;
            }
            
            formatTime(seconds) {
                const minutes = Math.floor(seconds / 60);
                const secs = Math.floor(seconds % 60);
                return `${minutes}:${secs.toString().padStart(2, '0')}`;
            }
        }
        
        const recording = {{ recording_data | safe }};
        const player = new TerminalPlayer(recording);
    </script>
</body>
</html>"#;

pub async fn export_web(
    input: PathBuf,
    output: Option<PathBuf>,
    theme: String,
    compress: bool,
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
        path.set_extension("html");
        path
    });
    
    let theme_colors = get_theme_colors(&theme);
    let recording_data = if compress {
        {
            let mut encoder = flate2::write::GzEncoder::new(
                Vec::new(),
                flate2::Compression::default()
            );
            encoder.write_all(&serde_json::to_vec(&recording)?)?;
            base64::encode(&encoder.finish()?)
        }
    } else {
        serde_json::to_string(&recording)?
    };
    
    let duration_formatted = format_duration(recording.duration);
    
    let mut env = Environment::new();
    env.add_template("player", HTML_TEMPLATE)?;
    let tmpl = env.get_template("player")?;
    
    let html = tmpl.render(context! {
        title => recording.title,
        duration => recording.duration,
        duration_formatted => duration_formatted,
        events_count => recording.events.len(),
        theme => theme_colors,
        recording_data => recording_data,
    })?;
    
    let mut output_file = File::create(&output_path)
        .context("Failed to create output file")?;
    
    output_file.write_all(html.as_bytes())
        .context("Failed to write HTML file")?;
    
    println!("Web export saved to: {}", output_path.display());
    println!("Duration: {:.1}s", recording.duration);
    println!("Events: {}", recording.events.len());
    println!("Theme: {}", theme);
    
    Ok(())
}

fn get_theme_colors(theme: &str) -> serde_json::Value {
    let terminal_theme = super::terminal_renderer::TerminalTheme::from_name(theme);
    
    serde_json::json!({
        "background": format!("rgb({}, {}, {})", terminal_theme.background.0, terminal_theme.background.1, terminal_theme.background.2),
        "foreground": format!("rgb({}, {}, {})", terminal_theme.foreground.0, terminal_theme.foreground.1, terminal_theme.foreground.2),
    })
}

fn format_duration(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{}:{:02}", minutes, seconds)
}