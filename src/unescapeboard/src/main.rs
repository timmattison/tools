use anyhow::Result;
use buildinfo::version_string;
use clipboardmon::{monitor_clipboard, Transformer, DEFAULT_POLL_INTERVAL};
use std::error::Error;

struct UnescapeTransformer;

impl Transformer for UnescapeTransformer {
    fn is_relevant(&self, content: &str) -> bool {
        // Check for escaped quotes
        content.contains(r#"\""#)
    }
    
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
        // Replace escaped quotes with regular quotes
        let unescaped = content.replace(r#"\""#, r#"""#);
        
        // Only return Ok if content actually changed
        if unescaped != content {
            Ok(unescaped)
        } else {
            Err("No escaped quotes found".into())
        }
    }
    
    fn waiting_message(&self) -> &str {
        "Waiting for escaped text in clipboard"
    }
    
    fn success_message(&self) -> &str {
        "Unescaped text in clipboard"
    }
}

fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("unescapeboard {}", version_string!());
        return Ok(());
    }

    env_logger::init();

    let transformer = UnescapeTransformer;
    monitor_clipboard(transformer, DEFAULT_POLL_INTERVAL)
}