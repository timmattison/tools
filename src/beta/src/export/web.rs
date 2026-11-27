use crate::Recording;
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use minijinja::{context, Environment};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use termstate::TerminalTheme;

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
    let recording = Recording::load(&input)?;

    let output_path = output.unwrap_or_else(|| {
        let mut path = input.clone();
        path.set_extension("html");
        path
    });

    // Get theme colors
    let theme_colors = WebTheme::named(&theme);

    // Prepare recording data
    let recording_data = if compress {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut encoder, &recording)
            .context("Failed to compress recording data")?;
        let compressed = encoder.finish().context("Failed to finish compression")?;
        format!(
            "JSON.parse(pako.inflate(Uint8Array.from(atob('{}'), c => c.charCodeAt(0)), {{ to: 'string' }}))",
            general_purpose::STANDARD.encode(&compressed)
        )
    } else {
        // Escape </ to <\/ to prevent </script> injection when embedding in HTML
        serde_json::to_string(&recording)
            .context("Failed to serialize recording")?
            .replace("</", r"<\/")
    };

    // Format duration
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the duration is a count of seconds; the saturating float cast clamps a negative or NaN duration from a corrupt recording to 0:00"
    )]
    let duration_total_seconds = recording.duration as u64;
    let duration_minutes = duration_total_seconds / 60;
    let duration_seconds = duration_total_seconds % 60;
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
    let mut output_file = File::create(&output_path).context("Failed to create output file")?;
    output_file
        .write_all(html.as_bytes())
        .context("Failed to write HTML file")?;

    println!("Web export saved to: {}", output_path.display());

    Ok(())
}

/// The colors of one theme, as the CSS hex strings (`#rrggbb`) that the HTML
/// template and xterm.js read.
///
/// The values come from the palettes of [`TerminalTheme`], so the web export and
/// the video export use one copy of each palette.
#[derive(Debug, serde::Serialize)]
struct WebTheme {
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

impl WebTheme {
    /// Gives the palette that `name` selects, as CSS hex strings.
    ///
    /// The web export matches a name exactly, and an unknown name gives the
    /// `auto` palette. [`TerminalTheme::from_name`] is not used, because it
    /// ignores case: `Dracula` gives the black palette here and the Dracula
    /// palette in the video export. Issue #768 keeps the HTML unchanged.
    fn named(name: &str) -> Self {
        let palette = match name {
            "dracula" => TerminalTheme::dracula(),
            "monokai" => TerminalTheme::monokai(),
            "solarized-dark" => TerminalTheme::solarized_dark(),
            "solarized-light" => TerminalTheme::solarized_light(),
            _ => TerminalTheme::auto(),
        };
        Self::from_palette(&palette)
    }

    /// Writes each color of `palette` as a CSS hex string.
    fn from_palette(palette: &TerminalTheme) -> Self {
        Self {
            background: css_hex(palette.background),
            foreground: css_hex(palette.foreground),
            black: css_hex(palette.black),
            red: css_hex(palette.red),
            green: css_hex(palette.green),
            yellow: css_hex(palette.yellow),
            blue: css_hex(palette.blue),
            magenta: css_hex(palette.magenta),
            cyan: css_hex(palette.cyan),
            white: css_hex(palette.white),
            bright_black: css_hex(palette.bright_black),
            bright_red: css_hex(palette.bright_red),
            bright_green: css_hex(palette.bright_green),
            bright_yellow: css_hex(palette.bright_yellow),
            bright_blue: css_hex(palette.bright_blue),
            bright_magenta: css_hex(palette.bright_magenta),
            bright_cyan: css_hex(palette.bright_cyan),
            bright_white: css_hex(palette.bright_white),
        }
    }
}

/// Writes an RGB color as a lowercase CSS hex string, for example `#ff5555`.
fn css_hex((red, green, blue): (u8, u8, u8)) -> String {
    format!("#{red:02x}{green:02x}{blue:02x}")
}
