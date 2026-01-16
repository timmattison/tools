use anyhow::{Context, Result};
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
    #[allow(dead_code, reason = "Required by GitHub API response structure but not used")]
    rate: RateLimit,
}

/// A rate limit paired with its resource name for display purposes
#[derive(Debug)]
struct NamedRateLimit<'a> {
    name: &'a str,
    rate_limit: &'a RateLimit,
}

/// Collects all rate limit resources into a vector for easier processing
fn collect_rate_limits(resources: &Resources) -> Vec<NamedRateLimit<'_>> {
    vec![
        NamedRateLimit { name: "core", rate_limit: &resources.core },
        NamedRateLimit { name: "graphql", rate_limit: &resources.graphql },
        NamedRateLimit { name: "search", rate_limit: &resources.search },
        NamedRateLimit { name: "code_search", rate_limit: &resources.code_search },
        NamedRateLimit { name: "code_scanning_upload", rate_limit: &resources.code_scanning_upload },
        NamedRateLimit { name: "code_scanning_autofix", rate_limit: &resources.code_scanning_autofix },
        NamedRateLimit { name: "actions_runner_registration", rate_limit: &resources.actions_runner_registration },
        NamedRateLimit { name: "integration_manifest", rate_limit: &resources.integration_manifest },
        NamedRateLimit { name: "source_import", rate_limit: &resources.source_import },
        NamedRateLimit { name: "dependency_snapshots", rate_limit: &resources.dependency_snapshots },
        NamedRateLimit { name: "dependency_sbom", rate_limit: &resources.dependency_sbom },
        NamedRateLimit { name: "scim", rate_limit: &resources.scim },
        NamedRateLimit { name: "audit_log", rate_limit: &resources.audit_log },
        NamedRateLimit { name: "audit_log_streaming", rate_limit: &resources.audit_log_streaming },
    ]
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

/// Prints a table of rate limits with the given title
/// Skips printing if the list is empty
fn print_rate_limit_table(title: &str, rate_limits: &[NamedRateLimit]) {
    if rate_limits.is_empty() {
        return;
    }

    println!("{}\n", title);
    println!(
        "{:<30} {:<8} {:<8} {:<10} Reset Time",
        "Resource", "Limit", "Used", "Remaining"
    );
    println!("{}", "─".repeat(79));

    for named in rate_limits {
        print_rate_limit_row(named.name, named.rate_limit);
    }

    println!();
}

/// Main entry point - fetches, parses, and displays GitHub API rate limits
/// Displays rate limits in two tables: available (non-exhausted) first, then exhausted
fn main() -> Result<()> {
    let json_data = fetch_rate_limit_data()?;
    let response: RateLimitResponse = serde_json::from_str(&json_data)
        .context("Failed to parse JSON response")?;

    // Print header
    let now = Local::now().format("%Y-%m-%d %H:%M:%S");
    println!("\nGitHub API Rate Limits (as of {})\n", now);

    // Collect and partition rate limits into available (remaining > 0) and exhausted (remaining == 0)
    let all_limits = collect_rate_limits(&response.resources);
    let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
        .into_iter()
        .partition(|named| named.rate_limit.remaining > 0);

    // Print available rate limits first (easier to scroll past)
    print_rate_limit_table("Available Rate Limits", &available);

    // Print exhausted rate limits last (easier to find at bottom of terminal)
    print_rate_limit_table("Exhausted Rate Limits", &exhausted);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a RateLimit with the given remaining count for testing
    fn make_rate_limit(remaining: u32) -> RateLimit {
        RateLimit {
            limit: 5000,
            used: 5000 - remaining,
            remaining,
            reset: Utc::now().timestamp() + 3600,
        }
    }

    /// Creates a Resources struct with specified remaining counts for core and graphql,
    /// all other resources get remaining=100
    fn make_resources(core_remaining: u32, graphql_remaining: u32) -> Resources {
        Resources {
            core: make_rate_limit(core_remaining),
            graphql: make_rate_limit(graphql_remaining),
            search: make_rate_limit(100),
            code_search: make_rate_limit(100),
            code_scanning_upload: make_rate_limit(100),
            code_scanning_autofix: make_rate_limit(100),
            actions_runner_registration: make_rate_limit(100),
            integration_manifest: make_rate_limit(100),
            source_import: make_rate_limit(100),
            dependency_snapshots: make_rate_limit(100),
            dependency_sbom: make_rate_limit(100),
            scim: make_rate_limit(100),
            audit_log: make_rate_limit(100),
            audit_log_streaming: make_rate_limit(100),
        }
    }

    #[test]
    fn test_collect_rate_limits_returns_all_14_resources() {
        let resources = make_resources(100, 100);
        let collected = collect_rate_limits(&resources);
        assert_eq!(collected.len(), 14, "Expected 14 rate limit resources");
    }

    #[test]
    fn test_collect_rate_limits_includes_expected_names() {
        let resources = make_resources(100, 100);
        let collected = collect_rate_limits(&resources);
        let names: Vec<&str> = collected.iter().map(|n| n.name).collect();

        let expected_names = [
            "core",
            "graphql",
            "search",
            "code_search",
            "code_scanning_upload",
            "code_scanning_autofix",
            "actions_runner_registration",
            "integration_manifest",
            "source_import",
            "dependency_snapshots",
            "dependency_sbom",
            "scim",
            "audit_log",
            "audit_log_streaming",
        ];

        for expected in expected_names {
            assert!(
                names.contains(&expected),
                "Expected resource '{}' not found in collected rate limits",
                expected
            );
        }
    }

    #[test]
    fn test_partition_separates_exhausted_from_available() {
        let resources = make_resources(0, 100); // core exhausted, graphql available
        let all_limits = collect_rate_limits(&resources);

        let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        // Core should be in exhausted
        assert!(
            exhausted.iter().any(|n| n.name == "core"),
            "Core (remaining=0) should be in exhausted list"
        );

        // Graphql should be in available
        assert!(
            available.iter().any(|n| n.name == "graphql"),
            "Graphql (remaining=100) should be in available list"
        );

        // Counts should add up
        assert_eq!(
            available.len() + exhausted.len(),
            14,
            "Total partitioned items should equal 14"
        );
    }

    #[test]
    fn test_partition_all_exhausted() {
        // Create resources where core and graphql are exhausted
        let mut resources = make_resources(0, 0);
        // Set all others to exhausted too
        resources.search = make_rate_limit(0);
        resources.code_search = make_rate_limit(0);
        resources.code_scanning_upload = make_rate_limit(0);
        resources.code_scanning_autofix = make_rate_limit(0);
        resources.actions_runner_registration = make_rate_limit(0);
        resources.integration_manifest = make_rate_limit(0);
        resources.source_import = make_rate_limit(0);
        resources.dependency_snapshots = make_rate_limit(0);
        resources.dependency_sbom = make_rate_limit(0);
        resources.scim = make_rate_limit(0);
        resources.audit_log = make_rate_limit(0);
        resources.audit_log_streaming = make_rate_limit(0);

        let all_limits = collect_rate_limits(&resources);
        let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        assert!(available.is_empty(), "All resources are exhausted, available should be empty");
        assert_eq!(exhausted.len(), 14, "All 14 resources should be exhausted");
    }

    #[test]
    fn test_partition_none_exhausted() {
        let resources = make_resources(100, 100); // All have remaining > 0
        let all_limits = collect_rate_limits(&resources);

        let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        assert!(exhausted.is_empty(), "No resources are exhausted");
        assert_eq!(available.len(), 14, "All 14 resources should be available");
    }

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
