use anyhow::Result;
use log::info;
use std::error::Error;
use std::thread;
use std::time::Duration;
use sysclip::Clipboard;

/// Trait for implementing clipboard content transformations
pub trait Transformer: Send {
    /// Check if the clipboard content is relevant for this transformer
    fn is_relevant(&self, content: &str) -> bool;

    /// Transform the content.
    ///
    /// # Errors
    ///
    /// Returns an error if the transformation fails (for example, an invalid
    /// input or an external operation reported by the implementor).
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

/// Monitor clipboard and apply transformations.
///
/// # Errors
///
/// Returns an error if the system clipboard cannot be opened, or — when the
/// transformer rejects the current contents — if those rejection errors should
/// be surfaced. Transient write failures are logged and the loop continues.
///
/// # The clipboard of the machine, never a command
///
/// The monitor opens the clipboard with [`Clipboard::open_native`], which never
/// runs `pbcopy` or `pbpaste`. A poll reads the clipboard each 500 ms. In a
/// mosh-rs session `pbpaste` asks the client for the clipboard, and the client
/// allows 5 pastes in each 10 seconds and shows each paste on its notification
/// bar. A poll through it thus fails, and it fills that bar. So the board tools
/// watch the clipboard of the machine they run on, as they did before.
pub fn monitor_clipboard<T: Transformer>(transformer: T, poll_interval: Duration) -> Result<()> {
    let mut clipboard = Clipboard::open_native()?;
    let mut last_seen = String::new();

    info!("{}, press CTRL-C to stop", transformer.waiting_message());

    loop {
        thread::sleep(poll_interval);

        // A clipboard that holds no text reads as an empty string, and a read
        // can fail for a moment. Neither is new content, so the poll skips both
        // and keeps what it saw last.
        let content = match clipboard.read() {
            Ok(text) if !text.is_empty() => text,
            _ => continue,
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
                    match clipboard.write(&transformed) {
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
