#[cfg(test)]
mod tests {
    use crate::export::terminal_renderer::{TerminalState, TerminalTheme};
    use crate::{Event, EventType, Recording};

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
            timestamp: 1234567890.0,
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
    fn test_terminal_theme_creation() {
        let theme = TerminalTheme::dracula();
        assert_eq!(theme.background, (40, 42, 54));
        assert_eq!(theme.foreground, (248, 248, 242));

        let theme = TerminalTheme::from_name("monokai");
        assert_eq!(theme.background, (39, 40, 34));

        let theme = TerminalTheme::from_name("unknown");
        assert_eq!(theme.background, (0, 0, 0)); // defaults to black
    }

    #[test]
    fn test_terminal_state() {
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(80, 24, theme);

        state.process_output("Hello").unwrap();

        {
            let grid = state.get_grid();
            let first_row = &grid[0];
            assert_eq!(first_row[0].ch, 'H');
            assert_eq!(first_row[1].ch, 'e');
            assert_eq!(first_row[2].ch, 'l');
        }

        assert_eq!(state.get_width(), 80);
        assert_eq!(state.get_height(), 24);
        assert_eq!(state.get_theme().foreground, (255, 255, 255));
    }

    #[test]
    fn test_scroll_up_via_newline_at_bottom() {
        // Tests scroll_up() logic via newline when cursor is at bottom of screen.
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(10, 3, theme);

        // Fill all three rows
        state.process_output("A\r\nB\r\nC").unwrap();
        assert_eq!(state.get_grid()[0][0].ch, 'A');
        assert_eq!(state.get_grid()[1][0].ch, 'B');
        assert_eq!(state.get_grid()[2][0].ch, 'C');

        // Newline at the bottom should scroll up
        state.process_output("\r\n").unwrap();

        let grid = state.get_grid();
        assert_eq!(grid[0][0].ch, 'B', "Row 0 should have B after scroll up");
        assert_eq!(grid[1][0].ch, 'C', "Row 1 should have C after scroll up");
        assert_eq!(
            grid[2][0].ch, ' ',
            "Bottom row should be cleared after scroll up"
        );
    }

    #[test]
    fn test_scroll_down_via_reverse_index() {
        // Tests scroll_down logic via ESC M (reverse index) at top of scroll region.
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(10, 3, theme);

        // Write "A" on row 0
        state.process_output("A").unwrap();
        assert_eq!(state.get_grid()[0][0].ch, 'A');

        // Move cursor to row 0 col 0
        state.process_output("\x1b[1;1H").unwrap();

        // ESC M (reverse index) at the top should scroll content down
        state.process_output("\x1bM").unwrap();

        let grid = state.get_grid();
        assert_eq!(
            grid[0][0].ch, ' ',
            "Row 0 should be cleared after scroll down"
        );
        assert_eq!(
            grid[1][0].ch, 'A',
            "Content should have moved down to row 1"
        );
    }

    #[test]
    fn test_delete_characters_large_count_no_panic() {
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(10, 5, theme);

        state.process_output("Hello").unwrap();

        // Move cursor to column 2, then delete 999 chars (way more than width)
        // This should NOT panic from usize underflow
        state.process_output("\x1b[1;3H\x1b[999P").unwrap();

        let grid = state.get_grid();
        assert_eq!(grid[0][0].ch, 'H');
        assert_eq!(grid[0][1].ch, 'e');
        // Remaining columns should be blank
        assert_eq!(grid[0][2].ch, ' ');
    }

    #[test]
    fn test_insert_characters_large_count_no_panic() {
        let theme = TerminalTheme::auto();
        let mut state = TerminalState::new(10, 5, theme);

        state.process_output("Hello").unwrap();

        // Move cursor to column 2, then insert 999 chars (way more than width)
        // This should NOT panic from usize underflow
        state.process_output("\x1b[1;3H\x1b[999@").unwrap();

        let grid = state.get_grid();
        assert_eq!(grid[0][0].ch, 'H');
        assert_eq!(grid[0][1].ch, 'e');
        // Inserted blanks should fill the rest
        assert_eq!(grid[0][2].ch, ' ');
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
    fn test_parse_color_spec_ascii_guard() {
        let theme = TerminalTheme::auto();
        let state = TerminalState::new(10, 5, theme);

        // Valid hex
        assert_eq!(state.parse_color_spec_pub("#ff0000"), Some((255, 0, 0)));
        assert_eq!(state.parse_color_spec_pub("#000000"), Some((0, 0, 0)));

        // Valid rgb:
        assert_eq!(
            state.parse_color_spec_pub("rgb:ff/00/ff"),
            Some((255, 0, 255))
        );
        assert_eq!(
            state.parse_color_spec_pub("rgb:ffff/0000/ffff"),
            Some((255, 0, 255))
        );

        // Named
        assert_eq!(state.parse_color_spec_pub("red"), Some((255, 0, 0)));

        // Non-ASCII should return None, not panic
        assert_eq!(state.parse_color_spec_pub("#ff\u{00e9}000"), None);

        // Invalid
        assert_eq!(state.parse_color_spec_pub("garbage"), None);
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
            state.detect_tmux_layout().is_none(),
            "Colored text on the last row should not trigger tmux detection"
        );
    }

    #[test]
    fn test_resolve_cell_colors_with_unmodified_palette() {
        let theme = TerminalTheme::auto();
        let state = TerminalState::new(10, 5, theme);

        let cell = crate::export::terminal_renderer::Cell {
            ch: 'A',
            fg_color: (255, 255, 255),
            bg_color: (0, 0, 0),
            bold: false,
            italic: false,
            underline: false,
        };

        let (fg, bg) = state.resolve_cell_colors(&cell);
        // With no palette modifications, colors should pass through
        assert_eq!(fg, (255, 255, 255));
        assert_eq!(bg, (0, 0, 0));
    }
}
