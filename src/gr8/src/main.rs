use anyhow::{Context, Result};
use buildinfo::version_string;
use chrono::{Local, TimeZone, Utc};
use colored::Colorize;
use serde::Deserialize;
use std::process::Command;

/// The GraphQL rate limit resource name. This is the most commonly monitored
/// rate limit, so it's sorted to appear last in output tables for visibility.
const GRAPHQL_RESOURCE: &str = "graphql";

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

/// A rate limit paired with its resource name for display purposes
#[derive(Debug)]
struct NamedRateLimit<'a> {
    name: &'a str,
    rate_limit: &'a RateLimit,
}

/// Macro that defines the Resources struct, RESOURCE_COUNT constant, and collect_rate_limits
/// function from a single list of field names. This ensures they cannot get out of sync.
///
/// When adding a new GitHub API rate limit resource:
/// 1. Add the field name to this macro invocation (in the order it appears in the API response)
/// 2. That's it! The struct, constant, and collection function are all updated automatically.
macro_rules! define_rate_limit_resources {
    ($($name:ident),* $(,)?) => {
        /// Contains all GitHub API rate limit resources
        #[derive(Debug, Deserialize)]
        struct Resources {
            $($name: RateLimit,)*
        }

        /// Collects all rate limit resources into a vector for easier processing.
        /// The returned vector contains all resources in the same order as defined in the struct.
        fn collect_rate_limits(resources: &Resources) -> Vec<NamedRateLimit<'_>> {
            vec![
                $(NamedRateLimit { name: stringify!($name), rate_limit: &resources.$name },)*
            ]
        }

        /// Number of rate limit resources defined in the GitHub API.
        /// This constant is automatically kept in sync with the Resources struct and collect_rate_limits.
        /// Only used in tests to verify the partitioning logic.
        #[cfg(test)]
        const RESOURCE_COUNT: usize = [$( stringify!($name) ),*].len();

        /// Creates a Resources struct for testing where all resources have the given remaining count.
        /// Uses the test module's make_rate_limit function.
        #[cfg(test)]
        fn make_all_resources_with_remaining(remaining: u32) -> Resources {
            use crate::tests::make_rate_limit;
            Resources {
                $($name: make_rate_limit(remaining),)*
            }
        }
    };
}

// Define all GitHub API rate limit resources in a single place.
// The macro generates: Resources struct, RESOURCE_COUNT constant, collect_rate_limits function,
// and the test helper make_all_resources_with_remaining.
define_rate_limit_resources! {
    core,
    graphql,
    search,
    code_search,
    code_scanning_upload,
    code_scanning_autofix,
    actions_runner_registration,
    integration_manifest,
    source_import,
    dependency_snapshots,
    dependency_sbom,
    scim,
    audit_log,
    audit_log_streaming,
}

/// Top-level response structure from GitHub API rate_limit endpoint
#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    resources: Resources,
    /// Rate limit for the core API (duplicates resources.core, kept for API structure completeness)
    #[allow(dead_code, reason = "Required by GitHub API response structure but not used")]
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

    // Collect and partition rate limits into available (remaining > 0) and exhausted (remaining == 0)
    let all_limits = collect_rate_limits(&response.resources);
    let (mut available, mut exhausted): (Vec<_>, Vec<_>) = all_limits
        .into_iter()
        .partition(|named| named.rate_limit.remaining > 0);

    // Sort each list so graphql appears last for visibility (most commonly monitored)
    // Uses sort_by_key: false < true in Rust, so graphql (where predicate is true) sorts last
    available.sort_by_key(|n| n.name == GRAPHQL_RESOURCE);
    exhausted.sort_by_key(|n| n.name == GRAPHQL_RESOURCE);

    // Print available rate limits first (easier to scroll past)
    print_rate_limit_table("Available Rate Limits", &available);

    // Print exhausted rate limits last (easier to find at bottom of terminal)
    print_rate_limit_table("Exhausted Rate Limits", &exhausted);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: RESOURCE_COUNT is defined at module level by the define_rate_limit_resources! macro,
    // ensuring it always matches the actual number of resources.

    /// Creates a RateLimit with the given remaining count for testing.
    /// This function is used by the macro-generated make_all_resources_with_remaining.
    pub fn make_rate_limit(remaining: u32) -> RateLimit {
        RateLimit {
            limit: 5000,
            used: 5000 - remaining,
            remaining,
            reset: Utc::now().timestamp() + 3600,
        }
    }

    /// Creates a Resources struct with specified remaining counts for core and graphql,
    /// all other resources get remaining=100. Useful for testing partitioning with specific resources.
    fn make_resources_with_specific_exhausted(core_remaining: u32, graphql_remaining: u32) -> Resources {
        let mut resources = make_all_resources_with_remaining(100);
        resources.core = make_rate_limit(core_remaining);
        resources.graphql = make_rate_limit(graphql_remaining);
        resources
    }

    #[test]
    fn test_collect_rate_limits_returns_all_resources() {
        let resources = make_all_resources_with_remaining(100);
        let collected = collect_rate_limits(&resources);
        assert_eq!(
            collected.len(),
            RESOURCE_COUNT,
            "collect_rate_limits returned {} items but RESOURCE_COUNT is {} \
             (both are generated by the same macro, so this should never fail)",
            collected.len(),
            RESOURCE_COUNT
        );
    }

    #[test]
    fn test_partition_separates_exhausted_from_available() {
        // core exhausted (0), graphql available (100), others available (100)
        let resources = make_resources_with_specific_exhausted(0, 100);
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
            RESOURCE_COUNT,
            "Total partitioned items should equal RESOURCE_COUNT"
        );
    }

    #[test]
    fn test_partition_all_exhausted() {
        // All resources exhausted (remaining=0)
        let resources = make_all_resources_with_remaining(0);

        let all_limits = collect_rate_limits(&resources);
        let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        assert!(
            available.is_empty(),
            "All resources are exhausted, available should be empty"
        );
        assert_eq!(
            exhausted.len(),
            RESOURCE_COUNT,
            "All resources should be exhausted"
        );
    }

    #[test]
    fn test_partition_none_exhausted() {
        // All resources available (remaining > 0)
        let resources = make_all_resources_with_remaining(100);
        let all_limits = collect_rate_limits(&resources);

        let (available, exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        assert!(exhausted.is_empty(), "No resources are exhausted");
        assert_eq!(
            available.len(),
            RESOURCE_COUNT,
            "All resources should be available"
        );
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

    #[test]
    fn test_graphql_sorted_last_in_available() {
        // All resources available
        let resources = make_all_resources_with_remaining(100);
        let all_limits = collect_rate_limits(&resources);
        let (mut available, _): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        // Apply the same sort used in main: false < true, so graphql sorts last
        available.sort_by_key(|n| n.name == GRAPHQL_RESOURCE);

        assert_eq!(
            available.last().map(|n| n.name),
            Some(GRAPHQL_RESOURCE),
            "{GRAPHQL_RESOURCE} should be last in available list"
        );
    }

    #[test]
    fn test_graphql_sorted_last_in_exhausted() {
        // All resources exhausted
        let resources = make_all_resources_with_remaining(0);
        let all_limits = collect_rate_limits(&resources);
        let (_, mut exhausted): (Vec<_>, Vec<_>) = all_limits
            .into_iter()
            .partition(|named| named.rate_limit.remaining > 0);

        // Apply the same sort used in main: false < true, so graphql sorts last
        exhausted.sort_by_key(|n| n.name == GRAPHQL_RESOURCE);

        assert_eq!(
            exhausted.last().map(|n| n.name),
            Some(GRAPHQL_RESOURCE),
            "{GRAPHQL_RESOURCE} should be last in exhausted list"
        );
    }
}
