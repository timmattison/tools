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
            chapters: Vec::new(),
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
            chapters: Vec::new(),
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
            chapters: Vec::new(),
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
}
