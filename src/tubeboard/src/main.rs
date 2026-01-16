use anyhow::Result;
use buildinfo::version_string;
use clipboardmon::{monitor_clipboard, Transformer, DEFAULT_POLL_INTERVAL};
use std::error::Error;
use url::Url;

struct TubeTransformer;

impl Transformer for TubeTransformer {
    fn is_relevant(&self, content: &str) -> bool {
        // Check for YouTube URLs
        content.contains("youtube.com") || content.contains("youtu.be")
    }
    
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
        // Parse the URL
        let url = Url::parse(content)?;
        
        // Extract video ID based on URL format
        let video_id = if url.host_str() == Some("youtu.be") {
            // Short URL format: https://youtu.be/VIDEO_ID
            url.path_segments()
                .and_then(|segments| segments.last())
                .filter(|id| !id.is_empty())
                .ok_or("No video ID in youtu.be URL")?
                .to_string()
        } else {
            // Standard format: https://www.youtube.com/watch?v=VIDEO_ID
            url.query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())
                .ok_or("No video ID parameter found")?
        };
        
        // Validate video ID (should be 11 characters)
        if video_id.len() != 11 {
            return Err("Invalid video ID length".into());
        }
        
        Ok(video_id)
    }
    
    fn waiting_message(&self) -> &str {
        "Waiting for YouTube URLs in clipboard"
    }
    
    fn success_message(&self) -> &str {
        "Extracted YouTube video ID"
    }
}

fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("tubeboard {}", version_string!());
        return Ok(());
    }

    env_logger::init();

    let transformer = TubeTransformer;
    monitor_clipboard(transformer, DEFAULT_POLL_INTERVAL)
}