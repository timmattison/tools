#[cfg(test)]
mod tests {
    use crate::{Event, EventType, Recording};
    use crate::export::terminal_renderer::TerminalTheme;
    
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
        assert_eq!(theme.background, (0, 43, 54)); // defaults to solarized_dark
    }
    
    #[test]
    fn test_terminal_state() {
        let theme = TerminalTheme::auto();
        let mut state = crate::export::terminal_renderer::TerminalState::new(80, 24, theme);
        
        state.process_output("Hello").unwrap();
        
        // Check that the grid contains the text
        {
            let grid = state.get_grid();
            let first_row = &grid[0];
            
            // Check first few characters
            assert_eq!(first_row[0].ch, 'H');
            assert_eq!(first_row[1].ch, 'e');
            assert_eq!(first_row[2].ch, 'l');
        }
        
        // Test that the terminal state properly tracks content
        // We can test the dimensions and theme access
        assert_eq!(state.get_width(), 80);
        assert_eq!(state.get_height(), 24);
        assert_eq!(state.get_theme().foreground, (131, 148, 150)); // solarized dark
    }
}