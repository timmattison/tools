use anyhow::{Context, Result};

/// Construct a UniFi cloud console URL from a console ID
pub fn build_console_url(console_id: &str, path: &str) -> String {
    format!("https://unifi.ui.com/consoles/{}/{}", console_id, path.trim_start_matches('/'))
}

/// Extract console ID from a UniFi cloud URL
pub fn extract_console_id(url: &str) -> Option<String> {
    let parts: Vec<&str> = url.split('/').collect();
    
    // Look for "consoles" in the path and get the next segment
    for (i, part) in parts.iter().enumerate() {
        if *part == "consoles" && i + 1 < parts.len() {
            return Some(parts[i + 1].to_string());
        }
    }
    
    None
}

/// Check if a URL is a UniFi cloud console URL
pub fn is_cloud_console_url(url: &str) -> bool {
    url.starts_with("https://unifi.ui.com/consoles/") || 
    url.starts_with("http://unifi.ui.com/consoles/")
}

/// Convert a cloud console URL to the corresponding API endpoint
pub fn cloud_url_to_api_endpoint(url: &str) -> Result<String> {
    if !is_cloud_console_url(url) {
        anyhow::bail!("Not a valid UniFi cloud console URL");
    }
    
    let console_id = extract_console_id(url)
        .context("Failed to extract console ID from URL")?;
    
    // For now, we'll return the console ID. In a full implementation,
    // this would need to resolve to the actual controller endpoint
    Ok(console_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_console_url() {
        let url = build_console_url("ABC123:456", "network/default/dashboard");
        assert_eq!(url, "https://unifi.ui.com/consoles/ABC123:456/network/default/dashboard");
    }

    #[test]
    fn test_extract_console_id() {
        let url = "https://unifi.ui.com/consoles/70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833/network/default/settings";
        let id = extract_console_id(url);
        assert_eq!(id, Some("70A741667C3000000000066DC7C00000000006BABC5A000000006289D202:1320847833".to_string()));
    }

    #[test]
    fn test_is_cloud_console_url() {
        assert!(is_cloud_console_url("https://unifi.ui.com/consoles/ABC123/network"));
        assert!(!is_cloud_console_url("https://192.168.1.1:8443"));
    }
}