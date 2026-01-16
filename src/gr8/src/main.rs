use anyhow::{Context, Result};
use buildinfo::version_string;
use chrono::{Local, TimeZone, Utc};
use colored::Colorize;
use serde::Deserialize;
use std::process::Command;

/// Represents a single rate limit resource with its limits and usage statistics
#[derive(Debug, Deserialize)]
struct RateLimit {
    /// Maximum number of requests allowed
    limit: u32,
    /// Number of requests already used
    used: u32,
    /// Number of requests remaining
    remaining: u32,
    /// Unix timestamp (epoch) when the rate limit resets
    reset: i64,
}

/// Contains all GitHub API rate limit resources
#[derive(Debug, Deserialize)]
struct Resources {
    core: RateLimit,
    search: RateLimit,
    graphql: RateLimit,
    integration_manifest: RateLimit,
    source_import: RateLimit,
    code_scanning_upload: RateLimit,
    code_scanning_autofix: RateLimit,
    actions_runner_registration: RateLimit,
    scim: RateLimit,
    dependency_snapshots: RateLimit,
    dependency_sbom: RateLimit,
    audit_log: RateLimit,
    audit_log_streaming: RateLimit,
    code_search: RateLimit,
}

/// Top-level response structure from GitHub API rate_limit endpoint
#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    resources: Resources,
    /// Rate limit for the core API (duplicates resources.core, kept for API structure completeness)
    #[allow(dead_code)]
    rate: RateLimit,
}

/// Executes the `gh api rate_limit` command and returns the JSON output
fn fetch_rate_limit_data() -> Result<String> {
    let output = Command::new("gh")
        .args(["api", "rate_limit"])
        .output()
        .context("Failed to execute 'gh' command. Is GitHub CLI installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("gh command failed: {}", stderr);
    }

    let stdout = String::from_utf8(output.stdout)
        .context("Failed to parse command output as UTF-8")?;

    Ok(stdout)
}

/// Converts a Unix epoch timestamp to a formatted local time string
/// Returns format: YYYY-MM-DD HH:MM:SS (local time, without timezone offset)
/// Returns "Invalid" if the timestamp cannot be parsed
fn format_reset_time(epoch: i64) -> String {
    match Local.timestamp_opt(epoch, 0).single() {
        Some(datetime) => datetime.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => "Invalid".to_string(),
    }
}

/// Calculates the time remaining until the reset timestamp and formats it as a human-readable string
/// Returns None if the reset time is in the past or invalid
/// Returns format like "1h 23m 45s" for times with hours, "5m 30s" for shorter durations
fn format_time_until_reset(epoch: i64) -> Option<String> {
    let now = Utc::now().timestamp();
    let remaining_seconds = epoch - now;

    if remaining_seconds <= 0 {
        return None;
    }

    let hours = remaining_seconds / 3600;
    let minutes = (remaining_seconds % 3600) / 60;
    let seconds = remaining_seconds % 60;

    if hours > 0 {
        Some(format!("{}h {}m {}s", hours, minutes, seconds))
    } else if minutes > 0 {
        Some(format!("{}m {}s", minutes, seconds))
    } else {
        Some(format!("{}s", seconds))
    }
}

/// Determines the appropriate color for a rate limit based on remaining percentage
/// Returns colored string for the remaining count with proper padding applied first.
/// Padding is applied before colorization to ensure proper column alignment.
/// - Red: No requests remaining (exceeded)
/// - Yellow: Less than 20% remaining
/// - Green: 20% or more remaining
fn colorize_remaining(rate_limit: &RateLimit) -> String {
    // Apply padding first (left-aligned, 10 characters wide)
    let remaining_str = format!("{:<10}", rate_limit.remaining);

    if rate_limit.remaining == 0 {
        remaining_str.red().to_string()
    } else {
        // Defensive check: avoid division by zero (though GitHub API should never return limit=0)
        let percentage = if rate_limit.limit > 0 {
            rate_limit.remaining as f64 / rate_limit.limit as f64
        } else {
            0.0
        };

        if percentage < 0.2 {
            remaining_str.yellow().to_string()
        } else {
            remaining_str.green().to_string()
        }
    }
}

/// Prints a formatted row for a single rate limit resource
/// When the rate limit is exceeded (remaining = 0), also displays the time until reset
fn print_rate_limit_row(name: &str, rate_limit: &RateLimit) {
    let reset_time = format_reset_time(rate_limit.reset);
    let remaining_colored = colorize_remaining(rate_limit);

    // Show time until reset when limit is exceeded
    let time_until_reset = if rate_limit.remaining == 0 {
        format_time_until_reset(rate_limit.reset)
            .map(|t| format!(" ({})", t))
            .unwrap_or_default()
    } else {
        String::new()
    };

    println!(
        "{:<30} {:<8} {:<8} {} {}{}",
        name,
        rate_limit.limit,
        rate_limit.used,
        remaining_colored,
        reset_time,
        time_until_reset
    );
}

/// Main entry point - fetches, parses, and displays GitHub API rate limits
fn main() -> Result<()> {
    // Handle --version flag
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!("gr8 {}", version_string!());
        return Ok(());
    }

    let json_data = fetch_rate_limit_data()?;
    let response: RateLimitResponse = serde_json::from_str(&json_data)
        .context("Failed to parse JSON response")?;

    // Print header
    let now = Local::now().format("%Y-%m-%d %H:%M:%S");
    println!("\nGitHub API Rate Limits (as of {})\n", now);

    // Print table header
    println!(
        "{:<30} {:<8} {:<8} {:<10} {}",
        "Resource", "Limit", "Used", "Remaining", "Reset Time"
    );
    println!("{}", "─".repeat(79));

    // Print all resource rate limits
    print_rate_limit_row("core", &response.resources.core);
    print_rate_limit_row("graphql", &response.resources.graphql);
    print_rate_limit_row("search", &response.resources.search);
    print_rate_limit_row("code_search", &response.resources.code_search);
    print_rate_limit_row("code_scanning_upload", &response.resources.code_scanning_upload);
    print_rate_limit_row("code_scanning_autofix", &response.resources.code_scanning_autofix);
    print_rate_limit_row("actions_runner_registration", &response.resources.actions_runner_registration);
    print_rate_limit_row("integration_manifest", &response.resources.integration_manifest);
    print_rate_limit_row("source_import", &response.resources.source_import);
    print_rate_limit_row("dependency_snapshots", &response.resources.dependency_snapshots);
    print_rate_limit_row("dependency_sbom", &response.resources.dependency_sbom);
    print_rate_limit_row("scim", &response.resources.scim);
    print_rate_limit_row("audit_log", &response.resources.audit_log);
    print_rate_limit_row("audit_log_streaming", &response.resources.audit_log_streaming);

    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_time_until_reset_past_returns_none() {
        let past_epoch = Utc::now().timestamp() - 100;
        assert_eq!(format_time_until_reset(past_epoch), None);
    }

    #[test]
    fn test_format_time_until_reset_zero_returns_none() {
        let now_epoch = Utc::now().timestamp();
        assert_eq!(format_time_until_reset(now_epoch), None);
    }

    #[test]
    fn test_format_time_until_reset_seconds_only() {
        let future_epoch = Utc::now().timestamp() + 45;
        assert_eq!(format_time_until_reset(future_epoch), Some("45s".to_string()));
    }

    #[test]
    fn test_format_time_until_reset_minutes_and_seconds() {
        let future_epoch = Utc::now().timestamp() + 130; // 2m 10s
        assert_eq!(format_time_until_reset(future_epoch), Some("2m 10s".to_string()));
    }

    #[test]
    fn test_format_time_until_reset_hours_minutes_seconds() {
        let future_epoch = Utc::now().timestamp() + 3665; // 1h 1m 5s
        assert_eq!(format_time_until_reset(future_epoch), Some("1h 1m 5s".to_string()));
    }

    #[test]
    fn test_format_time_until_reset_exact_hour() {
        let future_epoch = Utc::now().timestamp() + 3600; // 1h 0m 0s
        assert_eq!(format_time_until_reset(future_epoch), Some("1h 0m 0s".to_string()));
    }

    #[test]
    fn test_format_time_until_reset_exact_minute() {
        let future_epoch = Utc::now().timestamp() + 60; // 1m 0s
        assert_eq!(format_time_until_reset(future_epoch), Some("1m 0s".to_string()));
    }
}
