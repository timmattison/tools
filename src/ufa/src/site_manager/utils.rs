/// Check if a URL is a UniFi cloud console URL
pub fn is_cloud_console_url(url: &str) -> bool {
    url.starts_with("https://unifi.ui.com/consoles/") ||
    url.starts_with("http://unifi.ui.com/consoles/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cloud_console_url() {
        assert!(is_cloud_console_url("https://unifi.ui.com/consoles/ABC123/network"));
        assert!(!is_cloud_console_url("https://192.168.1.1:8443"));
    }
}
