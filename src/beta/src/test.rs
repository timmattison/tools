#[cfg(test)]
mod tests {
    use crate::{Event, EventType, Recording};
    use termstate::{TerminalState, TerminalTheme};

    #[test]
    fn test_event_serialization() {
        let event = Event {
            time: 1.5,
            event_type: EventType::Output,
            data: "test output".to_string(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"o\""));
        assert!(json.contains("\"time\":1.5"));
        assert!(json.contains("\"data\":\"test output\""));
    }

    #[test]
    fn test_recording_serialization() {
        let recording = Recording {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 1_234_567_890.0,
            duration: 10.5,
            command: "bash".to_string(),
            title: "Test recording".to_string(),
            env: std::collections::HashMap::new(),
            events: vec![
                Event {
                    time: 0.0,
                    event_type: EventType::Output,
                    data: "$ ".to_string(),
                },
                Event {
                    time: 1.0,
                    event_type: EventType::Input,
                    data: "ls\r".to_string(),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&recording).unwrap();
        assert!(json.contains("\"version\": 2"));
        assert!(json.contains("\"width\": 80"));
        assert!(json.contains("\"type\": \"o\""));
        assert!(json.contains("\"type\": \"i\""));
    }

    #[test]
    fn test_recording_load_json() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_beta_load.json");

        let recording = Recording {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 1000.0,
            duration: 5.0,
            command: "bash".to_string(),
            title: "Test".to_string(),
            env: std::collections::HashMap::new(),
            events: vec![Event {
                time: 0.0,
                event_type: EventType::Output,
                data: "hello".to_string(),
            }],
        };

        let json = serde_json::to_string(&recording).unwrap();
        std::fs::write(&path, &json).unwrap();

        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.width, 80);
        assert_eq!(loaded.events.len(), 1);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_recording_load_gzip() {
        use flate2::write::GzEncoder;

        let dir = std::env::temp_dir();
        // Deliberately NOT using .gz extension to test magic byte detection
        let path = dir.join("test_beta_load_compressed.bin");

        let recording = Recording {
            version: 2,
            width: 80,
            height: 24,
            timestamp: 1000.0,
            duration: 5.0,
            command: "bash".to_string(),
            title: "Compressed".to_string(),
            env: std::collections::HashMap::new(),
            events: vec![],
        };

        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = GzEncoder::new(file, flate2::Compression::default());
        serde_json::to_writer(&mut encoder, &recording).unwrap();
        encoder.finish().unwrap();

        // Should detect gzip via magic bytes despite non-.gz extension
        let loaded = Recording::load(&path).unwrap();
        assert_eq!(loaded.title, "Compressed");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_recording_load_empty_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_beta_empty.json");
        std::fs::write(&path, "").unwrap();

        let result = Recording::load(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_web_export_xss_escaping() {
        // Verify that </script> in recording data gets escaped
        let json = serde_json::to_string(&"</script><script>alert(1)</script>").unwrap();
        let escaped = json.replace("</", r"<\/");
        assert!(
            !escaped.contains("</script>"),
            "Should escape </script> tags"
        );
        assert!(escaped.contains(r"<\/script>"));
    }

    #[test]
    fn test_tmux_detection_no_false_positive_on_colored_text() {
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(80, 24, theme);

        // Write colored text on the last row (e.g., colored ls output)
        // This should NOT trigger tmux detection
        // CSI 32m = green foreground, then text, then reset
        state
            .process_output("\x1b[24;1H\x1b[32msome_file.txt\x1b[0m")
            .unwrap();

        assert!(
            crate::export::tmux::detect_tmux_layout(&state).is_none(),
            "Colored text on the last row should not trigger tmux detection"
        );
    }

    /// The terminal that `beta export video` builds for a recording whose
    /// header states `width` columns and `height` rows, after the terminal takes
    /// text, a tab, and a cursor position (CUP).
    ///
    /// The export passes the size of the header to `TerminalState::new` with no
    /// check, and a header is a value from outside the program.
    fn replay_header_of(width: u16, height: u16) -> TerminalState {
        let header = format!(
            r#"{{"version":2,"width":{width},"height":{height},"timestamp":0.0,"duration":0.0,"command":"bash","title":"","env":{{}},"events":[]}}"#
        );
        let recording: Recording = serde_json::from_str(&header).unwrap();
        let mut state = TerminalState::new(
            usize::from(recording.width),
            usize::from(recording.height),
            TerminalTheme::auto(),
        );
        state.process_output("hello").unwrap();
        state.process_output("\t").unwrap();
        state.process_output("\x1b[5;5H").unwrap();
        state
    }

    /// The size of the grid of `state`, as `(columns, rows)`.
    fn grid_size(state: &TerminalState) -> (usize, usize) {
        let grid = state.get_grid();
        (grid.first().map_or(0, Vec::len), grid.len())
    }

    #[test]
    fn a_header_of_zero_columns_gives_a_terminal_of_one_column() {
        let state = replay_header_of(0, 24);

        assert_eq!(grid_size(&state), (1, 24));
    }

    #[test]
    fn a_header_of_zero_rows_gives_a_terminal_of_one_row() {
        let state = replay_header_of(80, 0);

        assert_eq!(grid_size(&state), (80, 1));
    }
}
