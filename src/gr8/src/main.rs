use anyhow::{Context, Result};
use buildinfo::version_string;
use chrono::{Local, TimeZone, Utc};
use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};
use serde::Deserialize;
use std::process::Command;

/// The core rate limit resource name. Used in tests for consistency with other
/// resource name constants.
#[cfg(test)]
const CORE_RESOURCE: &str = "core";

/// The GraphQL rate limit resource name. This is the most commonly monitored
/// rate limit, so it's sorted to appear last in output tables for visibility.
const GRAPHQL_RESOURCE: &str = "graphql";

/// Sorts rate limits so graphql appears last for visibility (most commonly monitored).
///
/// Uses sort_by_key with a boolean predicate: false < true in Rust's Ord implementation,
/// so graphql (where predicate is true) sorts to the end while preserving relative order
/// of other elements (stable sort).
fn sort_graphql_last(rate_limits: &mut [NamedRateLimit]) {
    rate_limits.sort_by_key(|n| n.name == GRAPHQL_RESOURCE);
}

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
    format_duration_seconds(epoch - Utc::now().timestamp())
}

/// Formats a duration in seconds as a human-readable string
/// Returns None if the duration is zero or negative
/// Returns format like "1h 23m 45s" for times with hours, "5m 30s" for shorter durations
fn format_duration_seconds(remaining_seconds: i64) -> Option<String> {
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

/// Standard GitHub rate limit window size in seconds (1 hour)
const RATE_LIMIT_WINDOW_SECONDS: i64 = 3600;

/// Minimum elapsed time in seconds required to calculate a meaningful rate.
/// This prevents wildly inaccurate rates when the window just started.
const MIN_ELAPSED_FOR_RATE: i64 = 60;

/// Calculates the usage rate in requests per minute.
/// Returns None if not enough time has elapsed to calculate a meaningful rate,
/// or if the rate limit data is invalid (e.g., reset time in the past).
fn calculate_rate_per_minute(rate_limit: &RateLimit) -> Option<f64> {
    let now = Utc::now().timestamp();
    let time_until_reset = rate_limit.reset - now;

    // If reset time is in the past or at the edge, can't calculate meaningful rate
    if time_until_reset <= 0 {
        return None;
    }

    let time_elapsed = RATE_LIMIT_WINDOW_SECONDS - time_until_reset;

    // Need minimum elapsed time for meaningful rate calculation
    if time_elapsed < MIN_ELAPSED_FOR_RATE {
        return None;
    }

    // If nothing has been used, rate is 0
    if rate_limit.used == 0 {
        return Some(0.0);
    }

    let rate_per_second = rate_limit.used as f64 / time_elapsed as f64;
    Some(rate_per_second * 60.0) // Convert to per minute
}

/// Information about when/if a rate limit will be exhausted
#[derive(Debug, PartialEq)]
enum ExhaustionPrediction {
    /// Will exhaust in the given number of seconds from now
    WillExhaust(i64),
    /// Won't exhaust before reset (rate is sustainable)
    Sustainable,
    /// Can't predict (no usage yet or not enough data)
    Unknown,
}

/// Predicts when the rate limit will be exhausted at the current usage rate.
/// Returns ExhaustionPrediction indicating whether/when exhaustion will occur.
fn predict_exhaustion(rate_limit: &RateLimit) -> ExhaustionPrediction {
    // If already exhausted, no prediction needed
    if rate_limit.remaining == 0 {
        return ExhaustionPrediction::WillExhaust(0);
    }

    let rate_per_minute = match calculate_rate_per_minute(rate_limit) {
        Some(r) => r,
        None => return ExhaustionPrediction::Unknown,
    };

    // If no usage, won't exhaust
    if rate_per_minute <= 0.0 {
        return ExhaustionPrediction::Sustainable;
    }

    let time_to_exhaust_minutes = rate_limit.remaining as f64 / rate_per_minute;
    let time_to_exhaust_seconds = (time_to_exhaust_minutes * 60.0) as i64;

    let now = Utc::now().timestamp();
    let time_until_reset = rate_limit.reset - now;

    if time_to_exhaust_seconds < time_until_reset {
        ExhaustionPrediction::WillExhaust(time_to_exhaust_seconds)
    } else {
        ExhaustionPrediction::Sustainable
    }
}

/// Formats the rate for display
fn format_rate(rate_limit: &RateLimit) -> String {
    match calculate_rate_per_minute(rate_limit) {
        Some(rate) if rate >= 0.01 => format!("{:.1}/min", rate),
        Some(_) => "0/min".to_string(),
        None => "—".to_string(),
    }
}

/// Formats the exhaustion prediction and returns appropriate color
/// Returns (text, Option<Color>) for use with comfy-table
fn format_exhaustion_with_color(rate_limit: &RateLimit) -> (String, Option<Color>) {
    match predict_exhaustion(rate_limit) {
        ExhaustionPrediction::WillExhaust(0) => ("Now".to_string(), Some(Color::Red)),
        ExhaustionPrediction::WillExhaust(seconds) => {
            let formatted = format_duration_seconds(seconds).unwrap_or_else(|| "soon".to_string());
            (format!("in {}", formatted), Some(Color::Red))
        }
        ExhaustionPrediction::Sustainable => ("—".to_string(), Some(Color::Green)),
        ExhaustionPrediction::Unknown => ("—".to_string(), None),
    }
}

/// Determines the appropriate color for a rate limit based on remaining percentage
/// Returns (remaining_count, Color) for use with comfy-table
/// - Red: No requests remaining (exceeded)
/// - Yellow: Less than 20% remaining
/// - Green: 20% or more remaining
fn remaining_color(rate_limit: &RateLimit) -> Color {
    if rate_limit.remaining == 0 {
        Color::Red
    } else {
        let percentage = if rate_limit.limit > 0 {
            rate_limit.remaining as f64 / rate_limit.limit as f64
        } else {
            0.0
        };

        if percentage < 0.2 {
            Color::Yellow
        } else {
            Color::Green
        }
    }
}

/// Builds a table row for a single rate limit resource
fn build_rate_limit_row(named: &NamedRateLimit) -> Vec<Cell> {
    let rate_limit = named.rate_limit;
    let rate = format_rate(rate_limit);
    let (exhaustion_text, exhaustion_color) = format_exhaustion_with_color(rate_limit);
    let remaining_col = remaining_color(rate_limit);

    // Build reset time with optional time-until-reset for exhausted limits
    let reset_time = if rate_limit.remaining == 0 {
        let base = format_reset_time(rate_limit.reset);
        match format_time_until_reset(rate_limit.reset) {
            Some(t) => format!("{} ({})", base, t),
            None => base,
        }
    } else {
        format_reset_time(rate_limit.reset)
    };

    let exhaustion_cell = match exhaustion_color {
        Some(color) => Cell::new(exhaustion_text).fg(color),
        None => Cell::new(exhaustion_text),
    };

    vec![
        Cell::new(named.name),
        Cell::new(rate),
        exhaustion_cell,
        Cell::new(rate_limit.limit),
        Cell::new(rate_limit.used),
        Cell::new(rate_limit.remaining).fg(remaining_col),
        Cell::new(reset_time),
    ]
}

/// Prints a table of rate limits with the given title using comfy-table
/// Skips printing if the list is empty
fn print_rate_limit_table(title: &str, rate_limits: &[NamedRateLimit]) {
    if rate_limits.is_empty() {
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "Resource", "Rate", "Exhausts", "Limit", "Used", "Remaining", "Reset Time",
        ]);

    for named in rate_limits {
        table.add_row(build_rate_limit_row(named));
    }

    println!("{}\n", title);
    println!("{}", table);
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
    sort_graphql_last(&mut available);
    sort_graphql_last(&mut exhausted);

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
            exhausted.iter().any(|n| n.name == CORE_RESOURCE),
            "Core (remaining=0) should be in exhausted list"
        );

        // Graphql should be in available
        assert!(
            available.iter().any(|n| n.name == GRAPHQL_RESOURCE),
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

        // Use the same function as main() to ensure consistency
        sort_graphql_last(&mut available);

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

        // Use the same function as main() to ensure consistency
        sort_graphql_last(&mut exhausted);

        assert_eq!(
            exhausted.last().map(|n| n.name),
            Some(GRAPHQL_RESOURCE),
            "{GRAPHQL_RESOURCE} should be last in exhausted list"
        );
    }

    /// Creates a RateLimit for testing rate calculations with specific elapsed time and usage
    fn make_rate_limit_with_timing(used: u32, remaining: u32, seconds_until_reset: i64) -> RateLimit {
        RateLimit {
            limit: used + remaining,
            used,
            remaining,
            reset: Utc::now().timestamp() + seconds_until_reset,
        }
    }

    #[test]
    fn test_format_duration_seconds_positive() {
        assert_eq!(format_duration_seconds(45), Some("45s".to_string()));
        assert_eq!(format_duration_seconds(130), Some("2m 10s".to_string()));
        assert_eq!(format_duration_seconds(3665), Some("1h 1m 5s".to_string()));
    }

    #[test]
    fn test_format_duration_seconds_zero_or_negative() {
        assert_eq!(format_duration_seconds(0), None);
        assert_eq!(format_duration_seconds(-100), None);
    }

    #[test]
    fn test_calculate_rate_not_enough_elapsed_time() {
        // Only 30 seconds elapsed (3570 seconds until reset = 3600 - 30)
        let rate_limit = make_rate_limit_with_timing(100, 4900, 3570);
        assert_eq!(calculate_rate_per_minute(&rate_limit), None);
    }

    #[test]
    fn test_calculate_rate_reset_in_past() {
        let rate_limit = RateLimit {
            limit: 5000,
            used: 100,
            remaining: 4900,
            reset: Utc::now().timestamp() - 100,
        };
        assert_eq!(calculate_rate_per_minute(&rate_limit), None);
    }

    #[test]
    fn test_calculate_rate_zero_usage() {
        // 1800 seconds elapsed (30 min), 1800 until reset
        let rate_limit = make_rate_limit_with_timing(0, 5000, 1800);
        assert_eq!(calculate_rate_per_minute(&rate_limit), Some(0.0));
    }

    #[test]
    fn test_calculate_rate_normal_usage() {
        // 1800 seconds (30 min) elapsed, 600 requests used
        // Rate = 600 / 1800 = 0.333 per second = 20 per minute
        let rate_limit = make_rate_limit_with_timing(600, 4400, 1800);
        let rate = calculate_rate_per_minute(&rate_limit).unwrap();
        assert!((rate - 20.0).abs() < 0.1, "Expected ~20/min, got {}", rate);
    }

    #[test]
    fn test_predict_exhaustion_already_exhausted() {
        let rate_limit = make_rate_limit_with_timing(5000, 0, 1800);
        assert_eq!(
            predict_exhaustion(&rate_limit),
            ExhaustionPrediction::WillExhaust(0)
        );
    }

    #[test]
    fn test_predict_exhaustion_not_enough_data() {
        // Only 30 seconds elapsed, not enough for prediction
        let rate_limit = make_rate_limit_with_timing(100, 4900, 3570);
        assert_eq!(predict_exhaustion(&rate_limit), ExhaustionPrediction::Unknown);
    }

    #[test]
    fn test_predict_exhaustion_zero_usage() {
        // No usage = sustainable
        let rate_limit = make_rate_limit_with_timing(0, 5000, 1800);
        assert_eq!(
            predict_exhaustion(&rate_limit),
            ExhaustionPrediction::Sustainable
        );
    }

    #[test]
    fn test_predict_exhaustion_sustainable_rate() {
        // 30 min elapsed, used 600 out of 5000 (rate = 20/min)
        // 4400 remaining / 20 per min = 220 minutes to exhaust
        // But only 30 min (1800 sec) until reset, so sustainable
        let rate_limit = make_rate_limit_with_timing(600, 4400, 1800);
        assert_eq!(
            predict_exhaustion(&rate_limit),
            ExhaustionPrediction::Sustainable
        );
    }

    #[test]
    fn test_predict_exhaustion_will_exhaust() {
        // 30 min elapsed, used 4500 out of 5000 (rate = 150/min)
        // 500 remaining / 150 per min = 3.33 minutes to exhaust (~200 seconds)
        // 30 min (1800 sec) until reset, so will exhaust
        let rate_limit = make_rate_limit_with_timing(4500, 500, 1800);
        match predict_exhaustion(&rate_limit) {
            ExhaustionPrediction::WillExhaust(seconds) => {
                assert!(
                    seconds > 0 && seconds < 1800,
                    "Expected exhaustion before reset, got {} seconds",
                    seconds
                );
            }
            other => panic!("Expected WillExhaust, got {:?}", other),
        }
    }

    #[test]
    fn test_format_rate_with_usage() {
        // 30 min elapsed, 600 used = 20/min
        let rate_limit = make_rate_limit_with_timing(600, 4400, 1800);
        assert_eq!(format_rate(&rate_limit), "20.0/min");
    }

    #[test]
    fn test_format_rate_zero_usage() {
        let rate_limit = make_rate_limit_with_timing(0, 5000, 1800);
        assert_eq!(format_rate(&rate_limit), "0/min");
    }

    #[test]
    fn test_format_rate_not_enough_data() {
        // Only 30 seconds elapsed
        let rate_limit = make_rate_limit_with_timing(100, 4900, 3570);
        assert_eq!(format_rate(&rate_limit), "—");
    }
}
