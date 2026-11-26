use crate::Recording;
use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use minijinja::{context, Environment};
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

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
            position: relative;
            overflow: visible;
        }
        .progress-fill {
            height: 100%;
            border-radius: 2px;
            background-color: #007acc;
            width: 0%;
            transition: width 0.1s ease;
        }
        .chapter-marker {
            position: absolute;
            top: -4px;
            width: 4px;
            height: 12px;
            margin-left: -2px;
            padding: 0;
            border: none;
            border-radius: 1px;
            background-color: #f9e2af;
            cursor: pointer;
        }
        .chapter-marker:hover, .chapter-marker:focus-visible {
            background-color: #ffffff;
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
                <div id="chapter-markers"></div>
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
        function formatTime(seconds) {
            const minutes = Math.floor(seconds / 60);
            const rest = Math.floor(seconds % 60);
            return `${minutes}:${rest.toString().padStart(2, '0')}`;
        }

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
                
                this.renderChapterMarkers();
                this.bindEvents();
                this.updateDisplay();
            }

            renderChapterMarkers() {
                const chapters = Array.isArray(this.recording.chapters) ? this.recording.chapters : [];
                const duration = this.recording.duration;
                if (!(duration > 0)) return;
                const container = document.getElementById('chapter-markers');
                for (const chapter of chapters) {
                    // Like the video export, keep only a chapter inside the recording.
                    const time = chapter.time;
                    if (!(Number.isFinite(time) && time >= 0 && time <= duration)) continue;
                    const percent = time / duration * 100;
                    const label = `${chapter.label} (${formatTime(chapter.time)})`;
                    const marker = document.createElement('button');
                    marker.type = 'button';
                    marker.className = 'chapter-marker';
                    marker.style.left = `${percent}%`;
                    marker.title = label;
                    marker.setAttribute('aria-label', label);
                    marker.addEventListener('click', () => this.seekTo(chapter.time));
                    container.appendChild(marker);
                }
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
                
                this.currentTimeDisplay.textContent = formatTime(currentTime);
            }
        }
        
        const recording = {{ recording_data | safe }};
        const player = new TerminalPlayer(recording);
    </script>
</body>
</html>"#;

/// Build the HTML page of a recording.
///
/// # Errors
///
/// Returns an error when the recording data cannot be serialized or compressed,
/// or when the template cannot be rendered.
fn render_page(recording: &Recording, theme: &str, compress: bool) -> Result<String> {
    // Get theme colors
    let theme_colors = match theme {
        "dracula" => Theme::dracula(),
        "monokai" => Theme::monokai(),
        "solarized-dark" => Theme::solarized_dark(),
        "solarized-light" => Theme::solarized_light(),
        _ => Theme::auto(),
    };

    // Prepare recording data
    let recording_data = if compress {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        serde_json::to_writer(&mut encoder, recording)
            .context("Failed to compress recording data")?;
        let compressed = encoder.finish().context("Failed to finish compression")?;
        format!(
            "JSON.parse(pako.inflate(Uint8Array.from(atob('{}'), c => c.charCodeAt(0)), {{ to: 'string' }}))",
            general_purpose::STANDARD.encode(&compressed)
        )
    } else {
        // Write each `<` as `\u003c`. A `<` occurs only inside a JSON string,
        // where the escape gives the same text. Thus no `</script`, `<!--`, or
        // `<script` in the data can end the script or change how the HTML
        // tokenizer reads it.
        serde_json::to_string(recording)
            .context("Failed to serialize recording")?
            .replace('<', r"\u003c")
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
    template
        .render(context! {
            title => recording.title,
            duration => recording.duration,
            duration_formatted => duration_formatted,
            events_count => recording.events.len(),
            recording_data => recording_data,
            theme => theme_colors,
        })
        .context("Failed to render the page")
}

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

    let html = render_page(&recording, &theme, compress)?;

    // Write output file
    let mut output_file = File::create(&output_path).context("Failed to create output file")?;
    output_file
        .write_all(html.as_bytes())
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

#[cfg(test)]
mod tests {
    use super::render_page;
    use crate::{Chapter, Event, EventType, Recording};
    use base64::{engine::general_purpose, Engine as _};
    use std::io::Read;
    use std::path::PathBuf;

    const LABEL_JAPANESE: &str = "日本語テスト";
    const LABEL_CAFE: &str = "café 🎉";
    const LABEL_SCRIPT_END: &str = "</script><b>x";
    const LABEL_COMMENT_SCRIPT: &str = "<!--<script>";
    const THEME: &str = "auto";

    /// The smallest fakes that the player script needs when it loads.
    ///
    /// Each fake element is created on first use. It stores the listeners that
    /// the script adds, so the epilogue can fire a click.
    const NODE_PRELUDE: &str = r#"
function makeElement(tagName) {
    return {
        tagName,
        style: {},
        textContent: '',
        children: [],
        attributes: {},
        listeners: {},
        appendChild(child) { this.children.push(child); },
        addEventListener(type, fn) { this.listeners[type] = fn; },
        setAttribute(name, value) { this.attributes[name] = value; },
    };
}
const elementsById = {};
const document = {
    getElementById(id) {
        if (!elementsById[id]) { elementsById[id] = makeElement('div'); }
        return elementsById[id];
    },
    createElement(tag) { return makeElement(tag); },
    addEventListener() {},
};
const window = { addEventListener() {} };
class Terminal {
    loadAddon() {}
    open() {}
    write() {}
    reset() {}
    clear() {}
}
const FitAddon = { FitAddon: class { fit() {} } };
const requestAnimationFrame = () => 0;
const pako = {};
"#;

    /// Print the markers, click the second one, and print where the player stands.
    const NODE_EPILOGUE: &str = r#"
const markers = document.getElementById('chapter-markers').children;
console.log(JSON.stringify(markers.map((m) => ({
    left: m.style.left,
    title: m.title,
    ariaLabel: m.attributes['aria-label'],
    type: m.type,
    className: m.className,
}))));
markers[1].listeners.click();
console.log(String(player.pausedAt));
"#;

    fn recording(chapters: Vec<Chapter>) -> Recording {
        Recording {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 0.0,
            duration: 10.0,
            command: String::from("sh"),
            title: String::from("demo"),
            env: std::collections::HashMap::new(),
            events: Vec::new(),
            chapters,
        }
    }

    fn chapter(time: f64, label: &str) -> Chapter {
        Chapter {
            time,
            label: label.to_string(),
        }
    }

    fn two_chapters() -> Vec<Chapter> {
        vec![chapter(2.5, LABEL_JAPANESE), chapter(5.0, LABEL_CAFE)]
    }

    fn page(recording: &Recording, compress: bool) -> String {
        render_page(recording, THEME, compress).expect("the page renders")
    }

    /// Find the real `node` on the `PATH` of this test process.
    fn node() -> PathBuf {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path)
            .map(|dir| dir.join("node"))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| panic!("these tests need Node.js on PATH"))
    }

    /// Give the text of the last `<script>` element that has no `src`.
    fn inline_script(page: &str) -> &str {
        let open = "<script>";
        let (_, after) = page
            .rsplit_once(open)
            .expect("the page has an inline script");
        let (script, _) = after
            .split_once("</script>")
            .expect("the inline script ends");
        script
    }

    #[test]
    fn a_page_embeds_the_chapters_in_its_recording_data() {
        let html = page(&recording(two_chapters()), false);
        assert!(html.contains(r#""chapters":[{"time":"#), "no chapters key");
        assert!(html.contains(LABEL_JAPANESE));
        assert!(html.contains(LABEL_CAFE));
    }

    #[test]
    fn a_compressed_page_embeds_the_chapters_too() {
        let html = page(&recording(two_chapters()), true);
        let (_, after) = html.split_once("atob('").expect("atob call");
        let (encoded, _) = after.split_once("')").expect("end of base64");
        let packed = general_purpose::STANDARD
            .decode(encoded)
            .expect("valid base64");
        let mut json = String::new();
        flate2::read::GzDecoder::new(packed.as_slice())
            .read_to_string(&mut json)
            .expect("valid gzip");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let labels: Vec<&str> = value["chapters"]
            .as_array()
            .expect("chapters array")
            .iter()
            .filter_map(|c| c["label"].as_str())
            .collect();
        assert_eq!(labels, vec![LABEL_JAPANESE, LABEL_CAFE]);
    }

    #[test]
    fn a_label_cannot_end_the_script() {
        let html = page(&recording(vec![chapter(1.0, LABEL_SCRIPT_END)]), false);
        assert!(!html.contains("</script><b>"), "label ended the script");
    }

    #[test]
    fn a_label_cannot_put_the_script_into_an_escaped_state() {
        // In script data, `<!--` and then `<script` make the HTML tokenizer
        // read the closing `</script>` of the page as text.
        let html = page(&recording(vec![chapter(1.0, LABEL_COMMENT_SCRIPT)]), false);
        let (_, data) = html
            .split_once("const recording = ")
            .expect("the recording data");
        let (json, _) = data.split_once(";\n").expect("the end of the recording data");
        assert!(!json.contains("<!--"), "{json}");
        assert!(!json.to_ascii_lowercase().contains("<script"), "{json}");
        let value: serde_json::Value = serde_json::from_str(json).expect("valid json");
        assert_eq!(value["chapters"][0]["label"], LABEL_COMMENT_SCRIPT);
    }

    #[test]
    fn event_data_cannot_end_the_script_or_put_it_into_an_escaped_state() {
        let data = "</script><script>alert(1)</script><!--<script>";
        let mut with_event = recording(Vec::new());
        with_event.events.push(Event {
            time: 0.5,
            event_type: EventType::Output,
            data: data.to_string(),
        });
        let html = page(&with_event, false);
        let (_, after) = html
            .split_once("const recording = ")
            .expect("the recording data");
        let (json, _) = after
            .split_once(";\n")
            .expect("the end of the recording data");
        assert!(!json.contains('<'), "{json}");
        let value: serde_json::Value = serde_json::from_str(json).expect("valid json");
        assert_eq!(value["events"][0]["data"], data);
    }

    #[test]
    fn a_recording_with_no_chapter_embeds_no_chapters_key() {
        let html = page(&recording(Vec::new()), false);
        assert!(!html.contains(r#""chapters""#));
    }

    #[test]
    fn recording_json_without_chapters_loads_with_none() {
        let json = serde_json::to_string(&recording(Vec::new())).expect("serializes");
        assert!(!json.contains("chapters"));
        let loaded: Recording = serde_json::from_str(&json).expect("loads");
        assert!(loaded.chapters.is_empty());
    }

    #[test]
    fn recording_json_with_chapters_loads_them() {
        let json = serde_json::to_string(&recording(two_chapters())).expect("serializes");
        let loaded: Recording = serde_json::from_str(&json).expect("loads");
        assert_eq!(loaded.chapters.len(), 2);
        assert_eq!(loaded.chapters[1].label, LABEL_CAFE);
        assert!((loaded.chapters[0].time - 2.5).abs() < f64::EPSILON);
    }

    /// Run the player of the page of `recording` under the real `node`. Give
    /// each marker that it built, and where the player stands after a click on
    /// the second marker.
    fn run_player(recording: &Recording) -> (Vec<serde_json::Value>, String) {
        let html = page(recording, false);
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("player.js");
        let program = format!("{NODE_PRELUDE}\n{}\n{NODE_EPILOGUE}", inline_script(&html));
        std::fs::write(&file, program).expect("write the program");

        let output = childenv::fixture_command(node())
            .arg(&file)
            .output()
            .expect("run node");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "node failed: {stderr}");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let markers: serde_json::Value =
            serde_json::from_str(lines.next().expect("marker line")).expect("marker json");
        let markers = markers.as_array().expect("marker array").clone();
        let paused_at = lines.next().expect("position line").to_owned();
        (markers, paused_at)
    }

    #[test]
    fn the_player_builds_one_marker_for_each_chapter_under_node() {
        let (markers, paused_at) =
            run_player(&recording(vec![chapter(2.5, "Setup"), chapter(5.0, "Build")]));
        assert_eq!(markers.len(), 2);
        let lefts: Vec<&str> = markers.iter().filter_map(|m| m["left"].as_str()).collect();
        assert_eq!(lefts, vec!["25%", "50%"]);
        let titles: Vec<&str> = markers.iter().filter_map(|m| m["title"].as_str()).collect();
        assert_eq!(titles, vec!["Setup (0:02)", "Build (0:05)"]);
        let aria: Vec<&str> = markers
            .iter()
            .filter_map(|m| m["ariaLabel"].as_str())
            .collect();
        assert_eq!(aria, titles);
        assert!(markers.iter().all(|m| m["type"] == "button"));
        assert!(markers.iter().all(|m| m["className"] == "chapter-marker"));
        assert_eq!(paused_at, "5");
    }

    #[test]
    fn the_player_makes_no_marker_for_a_chapter_outside_the_recording() {
        // JSON has no NaN, thus the time of the chapter "nan" is null in the page.
        let (markers, paused_at) = run_player(&recording(vec![
            chapter(-1.0, "before"),
            chapter(2.5, "Setup"),
            chapter(f64::NAN, "nan"),
            chapter(5.0, "Build"),
            chapter(15.0, "after"),
        ]));
        let titles: Vec<&str> = markers.iter().filter_map(|m| m["title"].as_str()).collect();
        assert_eq!(titles, vec!["Setup (0:02)", "Build (0:05)"]);
        assert_eq!(paused_at, "5");
    }
}
