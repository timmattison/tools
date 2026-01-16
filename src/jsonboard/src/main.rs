use anyhow::Result;
use buildinfo::version_string;
use clipboardmon::{monitor_clipboard, Transformer, DEFAULT_POLL_INTERVAL};
use serde_json::Value;
use std::error::Error;

struct JsonTransformer;

impl Transformer for JsonTransformer {
    fn is_relevant(&self, content: &str) -> bool {
        // Quick check for JSON-like content
        content.contains('{') || content.contains('}') || 
        content.contains('[') || content.contains(']') || 
        content.contains('"')
    }
    
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
        // Parse JSON to validate
        let value: Value = serde_json::from_str(content)?;
        
        // Pretty print with 3-space indentation (matching Go version)
        let formatted = serde_json::to_string_pretty(&value)?;
        
        Ok(formatted)
    }
    
    fn waiting_message(&self) -> &str {
        "Waiting for JSON in clipboard"
    }
    
    fn success_message(&self) -> &str {
        "Reformatted JSON in clipboard"
    }
}

fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("jsonboard {}", version_string!());
        return Ok(());
    }

    env_logger::init();

    let transformer = JsonTransformer;
    monitor_clipboard(transformer, DEFAULT_POLL_INTERVAL)
}