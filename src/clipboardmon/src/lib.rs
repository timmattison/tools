use anyhow::Result;
use arboard::Clipboard;
use log::info;
use std::error::Error;
use std::thread;
use std::time::Duration;

/// Trait for implementing clipboard content transformations
pub trait Transformer: Send {
    /// Check if the clipboard content is relevant for this transformer
    fn is_relevant(&self, content: &str) -> bool;
    
    /// Transform the content
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>>;
    
    /// Get the waiting message to display at startup
    fn waiting_message(&self) -> &str {
        "Waiting for content in clipboard"
    }
    
    /// Get the success message to display after transformation
    fn success_message(&self) -> &str {
        "Transformed clipboard content"
    }
}

/// Monitor clipboard and apply transformations
pub fn monitor_clipboard<T: Transformer>(
    transformer: T,
    poll_interval: Duration,
) -> Result<()> {
    let mut clipboard = Clipboard::new()?;
    let mut last_seen = String::new();
    
    info!("{}, press CTRL-C to stop", transformer.waiting_message());
    
    loop {
        thread::sleep(poll_interval);
        
        // Try to read clipboard content
        let content = match clipboard.get_text() {
            Ok(text) => text,
            Err(_) => continue, // Clipboard might be empty or contain non-text
        };
        
        // Skip if content hasn't changed
        if content == last_seen {
            continue;
        }
        
        last_seen = content.clone();
        
        // Check if content is relevant
        if !transformer.is_relevant(&content) {
            continue;
        }
        
        // Transform the content
        match transformer.transform(&content) {
            Ok(transformed) => {
                // Only update clipboard if content actually changed
                if transformed != content {
                    match clipboard.set_text(&transformed) {
                        Ok(_) => {
                            info!("{}", transformer.success_message());
                            last_seen = transformed;
                        }
                        Err(e) => {
                            log::error!("Failed to write to clipboard: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                log::debug!("Transformation failed: {}", e);
                // Don't log errors for invalid content, just continue monitoring
            }
        }
    }
}

/// Default poll interval (500ms to match Go version)
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

#[cfg(test)]
mod tests {
    use super::*;
    
    struct TestTransformer;
    
    impl Transformer for TestTransformer {
        fn is_relevant(&self, content: &str) -> bool {
            content.contains("test")
        }
        
        fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
            Ok(content.to_uppercase())
        }
    }
    
    #[test]
    fn test_transformer_trait() {
        let transformer = TestTransformer;
        
        assert!(transformer.is_relevant("test content"));
        assert!(!transformer.is_relevant("other content"));
        
        let result = transformer.transform("test content").unwrap();
        assert_eq!(result, "TEST CONTENT");
    }
}