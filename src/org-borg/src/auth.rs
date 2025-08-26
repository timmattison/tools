use anyhow::Result;
use std::process::Command;

/// Get GitHub token from gh CLI if available
pub fn get_gh_token() -> Result<Option<String>> {
    let output = Command::new("gh")
        .args(&["auth", "token"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();
            
            if !token.is_empty() {
                Ok(Some(token))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Get GitHub authentication status from gh CLI
pub fn get_gh_auth_status() -> Result<Option<String>> {
    let output = Command::new("gh")
        .args(&["auth", "status"])
        .output();

    match output {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            
            // gh auth status writes to stderr for some reason
            let status = if !stderr.is_empty() {
                stderr.to_string()
            } else {
                stdout.to_string()
            };
            
            if !status.is_empty() {
                Ok(Some(status))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Check if gh CLI is installed
pub fn is_gh_installed() -> bool {
    Command::new("gh")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}