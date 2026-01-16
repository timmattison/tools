use anyhow::Result;
use buildinfo::version_string;
use clipboardmon::{monitor_clipboard, Transformer, DEFAULT_POLL_INTERVAL};
use std::error::Error;

struct HtmlTransformer;

impl Transformer for HtmlTransformer {
    fn is_relevant(&self, content: &str) -> bool {
        // Quick check for HTML-like content
        content.contains('<') && content.contains('>')
    }
    
    fn transform(&self, content: &str) -> Result<String, Box<dyn Error>> {
        // Simple HTML pretty-printing
        let formatted = pretty_print_html(content);
        Ok(formatted)
    }
    
    fn waiting_message(&self) -> &str {
        "Waiting for HTML in clipboard"
    }
    
    fn success_message(&self) -> &str {
        "Reformatted HTML in clipboard"
    }
}

/// Simple HTML pretty printer
fn pretty_print_html(html: &str) -> String {
    let mut result = String::new();
    let mut indent: usize = 0;
    let mut in_tag = false;
    let mut tag_content = String::new();
    
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    
    while i < chars.len() {
        match chars[i] {
            '<' => {
                // Trim whitespace before tag
                let trimmed = result.trim_end();
                result.truncate(trimmed.len());
                
                in_tag = true;
                tag_content.clear();
                tag_content.push('<');
                
                // Look ahead to determine tag type
                if i + 1 < chars.len() && chars[i + 1] == '/' {
                    // Closing tag
                    indent = indent.saturating_sub(1);
                }
                
                // Add newline and indentation if not at start
                if !result.is_empty() {
                    result.push('\n');
                    result.push_str(&"    ".repeat(indent));
                }
            }
            '>' => {
                tag_content.push('>');
                result.push_str(&tag_content);
                in_tag = false;
                
                // Check if this was an opening tag (not self-closing or closing tag)
                let is_closing = tag_content.starts_with("</");
                let is_self_closing = tag_content.ends_with("/>");
                let is_void = is_void_element(&tag_content);
                
                if !is_closing && !is_self_closing && !is_void {
                    indent += 1;
                }
            }
            _ => {
                if in_tag {
                    tag_content.push(chars[i]);
                } else {
                    // Skip leading whitespace after tags
                    if !chars[i].is_whitespace() || !result.ends_with('>') {
                        result.push(chars[i]);
                    }
                }
            }
        }
        i += 1;
    }
    
    result.trim().to_string()
}

/// Check if the tag is a void element (self-closing in HTML5)
fn is_void_element(tag: &str) -> bool {
    let void_elements = [
        "area", "base", "br", "col", "embed", "hr", "img", "input",
        "link", "meta", "param", "source", "track", "wbr"
    ];
    
    void_elements.iter().any(|&elem| {
        tag.contains(&format!("<{}", elem)) || tag.contains(&format!("<{} ", elem))
    })
}

fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("htmlboard {}", version_string!());
        return Ok(());
    }

    env_logger::init();

    let transformer = HtmlTransformer;
    monitor_clipboard(transformer, DEFAULT_POLL_INTERVAL)
}
