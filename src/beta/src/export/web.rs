use anyhow::{Context, Result};
use minijinja::{Environment, context};
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use base64::{Engine as _, engine::general_purpose};
use crate::Recording;

const HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{{ title }}</title>
    <!-- xterm.js -->
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/xterm@5.3.0/css/xterm.css" />
    <script src="https://cdn.jsdelivr.net/npm/xterm@5.3.0/lib/xterm.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/xterm-addon-fit@0.8.0/lib/xterm-addon-fit.js"></script>
    <!-- pako for decompression if needed -->
    <script src="https://cdn.jsdelivr.net/npm/pako@2.1.0/dist/pako.min.js"></script>
    <style>
        body {
            margin: 0;
            padding: 20px;
            background-color: #1e1e1e;
            color: #ffffff;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
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
        #terminal {
            width: 100%;
            height: 600px;
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
            <div id="terminal"></div>
            
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
                
                // Initialize xterm.js
                this.terminal = new Terminal({
                    cols: recording.width,
                    rows: recording.height,
                    theme: {
                        background: '{{ theme.background }}',
                        foreground: '{{ theme.foreground }}',
                        cursor: '{{ theme.foreground }}',
                        black: '{{ theme.black }}',
                        red: '{{ theme.red }}',
                        green: '{{ theme.green }}',
                        yellow: '{{ theme.yellow }}',
                        blue: '{{ theme.blue }}',
                        magenta: '{{ theme.magenta }}',
                        cyan: '{{ theme.cyan }}',
                        white: '{{ theme.white }}',
                        brightBlack: '{{ theme.bright_black }}',
                        brightRed: '{{ theme.bright_red }}',
                        brightGreen: '{{ theme.bright_green }}',
                        brightYellow: '{{ theme.bright_yellow }}',
                        brightBlue: '{{ theme.bright_blue }}',
                        brightMagenta: '{{ theme.bright_magenta }}',
                        brightCyan: '{{ theme.bright_cyan }}',
                        brightWhite: '{{ theme.bright_white }}'
                    },
                    fontFamily: '"JetBrains Mono", "Monaco", "Menlo", "Ubuntu Mono", monospace',
                    fontSize: 14,
                    lineHeight: 1.2,
                    cursorBlink: false,
                    disableStdin: true,
                    allowProposedApi: true
                });
                
                // Initialize fit addon
                this.fitAddon = new FitAddon.FitAddon();
                this.terminal.loadAddon(this.fitAddon);
                
                // Open terminal in the DOM
                this.terminal.open(document.getElementById('terminal'));
                this.fitAddon.fit();
                
                // Get DOM elements
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
                
                // Handle window resize
                window.addEventListener('resize', () => {
                    this.fitAddon.fit();
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
                this.pausedAt = this.getCurrentTime();
            }
            
            restart() {
                const wasPlaying = this.isPlaying;
                this.pause();
                this.currentEventIndex = 0;
                this.pausedAt = 0;
                this.terminal.reset();
                this.terminal.clear();
                this.updateDisplay();
                if (wasPlaying) {
                    this.play();
                }
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
                this.terminal.reset();
                this.terminal.clear();
                
                // Replay all events up to the target time
                for (let i = 0; i < this.events.length; i++) {
                    if (this.events[i].time <= time) {
                        if (this.events[i].type === 'o') {
                            this.terminal.write(this.events[i].data);
                        }
                        this.currentEventIndex = i + 1;
                    } else {
                        break;
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
                        this.terminal.write(event.data);
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
    
    // Get theme colors
    let theme_colors = match theme.as_str() {
        "dracula" => Theme::dracula(),
        "monokai" => Theme::monokai(),
        "solarized-dark" => Theme::solarized_dark(),
        "solarized-light" => Theme::solarized_light(),
        _ => Theme::auto(),
    };
    
    // Prepare recording data
    let recording_data = if compress {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut encoder, &recording)
            .context("Failed to compress recording data")?;
        let compressed = encoder.finish()
            .context("Failed to finish compression")?;
        format!(
            "JSON.parse(pako.inflate(Uint8Array.from(atob('{}'), c => c.charCodeAt(0)), {{ to: 'string' }}))",
            general_purpose::STANDARD.encode(&compressed)
        )
    } else {
        serde_json::to_string(&recording)
            .context("Failed to serialize recording")?
    };
    
    // Format duration
    let duration_minutes = recording.duration as u64 / 60;
    let duration_seconds = recording.duration as u64 % 60;
    let duration_formatted = format!("{}:{:02}", duration_minutes, duration_seconds);
    
    // Create template environment
    let mut env = Environment::new();
    env.add_template("main", HTML_TEMPLATE)?;
    let template = env.get_template("main")?;
    
    // Render HTML
    let html = template.render(context! {
        title => recording.title,
        duration => recording.duration,
        duration_formatted => duration_formatted,
        events_count => recording.events.len(),
        recording_data => recording_data,
        theme => theme_colors,
    })?;
    
    // Write output file
    let mut output_file = File::create(&output_path)
        .context("Failed to create output file")?;
    output_file.write_all(html.as_bytes())
        .context("Failed to write HTML file")?;
    
    println!("Web export saved to: {}", output_path.display());
    
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct Theme {
    background: String,
    foreground: String,
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    white: String,
    bright_black: String,
    bright_red: String,
    bright_green: String,
    bright_yellow: String,
    bright_blue: String,
    bright_magenta: String,
    bright_cyan: String,
    bright_white: String,
}

impl Theme {
    fn auto() -> Self {
        Self::black()
    }
    
    fn black() -> Self {
        Self {
            background: String::from("#000000"),
            foreground: String::from("#ffffff"),
            black: String::from("#000000"),
            red: String::from("#ff5555"),
            green: String::from("#55ff55"),
            yellow: String::from("#ffff55"),
            blue: String::from("#5555ff"),
            magenta: String::from("#ff55ff"),
            cyan: String::from("#55ffff"),
            white: String::from("#ffffff"),
            bright_black: String::from("#555555"),
            bright_red: String::from("#ff5555"),
            bright_green: String::from("#55ff55"),
            bright_yellow: String::from("#ffff55"),
            bright_blue: String::from("#5555ff"),
            bright_magenta: String::from("#ff55ff"),
            bright_cyan: String::from("#55ffff"),
            bright_white: String::from("#ffffff"),
        }
    }
    
    fn dracula() -> Self {
        Self {
            background: String::from("#282a36"),
            foreground: String::from("#f8f8f2"),
            black: String::from("#282a36"),
            red: String::from("#ff5555"),
            green: String::from("#50fa7b"),
            yellow: String::from("#f1fa8c"),
            blue: String::from("#6272a4"),
            magenta: String::from("#ff79c6"),
            cyan: String::from("#8be9fd"),
            white: String::from("#f8f8f2"),
            bright_black: String::from("#6272a4"),
            bright_red: String::from("#ff5555"),
            bright_green: String::from("#50fa7b"),
            bright_yellow: String::from("#f1fa8c"),
            bright_blue: String::from("#6272a4"),
            bright_magenta: String::from("#ff79c6"),
            bright_cyan: String::from("#8be9fd"),
            bright_white: String::from("#ffffff"),
        }
    }
    
    fn monokai() -> Self {
        Self {
            background: String::from("#272822"),
            foreground: String::from("#f8f8f2"),
            black: String::from("#272822"),
            red: String::from("#f92672"),
            green: String::from("#a6e22e"),
            yellow: String::from("#f4bf75"),
            blue: String::from("#66d9ef"),
            magenta: String::from("#ae81ff"),
            cyan: String::from("#a1efe4"),
            white: String::from("#f8f8f2"),
            bright_black: String::from("#75715e"),
            bright_red: String::from("#f92672"),
            bright_green: String::from("#a6e22e"),
            bright_yellow: String::from("#f4bf75"),
            bright_blue: String::from("#66d9ef"),
            bright_magenta: String::from("#ae81ff"),
            bright_cyan: String::from("#a1efe4"),
            bright_white: String::from("#f8f8f2"),
        }
    }
    
    fn solarized_dark() -> Self {
        Self {
            background: String::from("#002b36"),
            foreground: String::from("#839496"),
            black: String::from("#073642"),
            red: String::from("#dc322f"),
            green: String::from("#859900"),
            yellow: String::from("#b58900"),
            blue: String::from("#268bd2"),
            magenta: String::from("#d33682"),
            cyan: String::from("#2aa198"),
            white: String::from("#eee8d5"),
            bright_black: String::from("#002b36"),
            bright_red: String::from("#cb4b16"),
            bright_green: String::from("#586e75"),
            bright_yellow: String::from("#657b83"),
            bright_blue: String::from("#839496"),
            bright_magenta: String::from("#6c71c4"),
            bright_cyan: String::from("#93a1a1"),
            bright_white: String::from("#fdf6e3"),
        }
    }
    
    fn solarized_light() -> Self {
        Self {
            background: String::from("#fdf6e3"),
            foreground: String::from("#657b83"),
            black: String::from("#073642"),
            red: String::from("#dc322f"),
            green: String::from("#859900"),
            yellow: String::from("#b58900"),
            blue: String::from("#268bd2"),
            magenta: String::from("#d33682"),
            cyan: String::from("#2aa198"),
            white: String::from("#eee8d5"),
            bright_black: String::from("#002b36"),
            bright_red: String::from("#cb4b16"),
            bright_green: String::from("#586e75"),
            bright_yellow: String::from("#657b83"),
            bright_blue: String::from("#839496"),
            bright_magenta: String::from("#6c71c4"),
            bright_cyan: String::from("#93a1a1"),
            bright_white: String::from("#fdf6e3"),
        }
    }
}