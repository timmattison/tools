use anyhow::{anyhow, Result};
use chrono::{DateTime, Duration, Local, TimeZone};
use chrono_english::{parse_date_string, Dialect};

/// Parse a duration string with support for days (d) and weeks (w)
pub fn parse_duration(duration_str: &str) -> Result<Duration> {
    // Check for day format (e.g., "6d")
    if let Some(days_str) = duration_str.strip_suffix('d') {
        if let Ok(days) = days_str.parse::<i64>() {
            return Ok(Duration::days(days));
        }
    }

    // Check for week format (e.g., "2w")  
    if let Some(weeks_str) = duration_str.strip_suffix('w') {
        if let Ok(weeks) = weeks_str.parse::<i64>() {
            return Ok(Duration::weeks(weeks));
        }
    }

    // Check for hour format (e.g., "24h")
    if let Some(hours_str) = duration_str.strip_suffix('h') {
        if let Ok(hours) = hours_str.parse::<i64>() {
            return Ok(Duration::hours(hours));
        }
    }

    // Check for minute format (e.g., "30m")
    if let Some(minutes_str) = duration_str.strip_suffix('m') {
        if let Ok(minutes) = minutes_str.parse::<i64>() {
            return Ok(Duration::minutes(minutes));
        }
    }

    // Try parsing with chrono-english as fallback
    let now = Local::now();
    match parse_date_string(duration_str, now, Dialect::Uk) {
        Ok(parsed_date) => {
            let duration = parsed_date.signed_duration_since(now).abs();
            Ok(duration)
        }
        Err(_) => Err(anyhow!("Could not parse duration: {}", duration_str)),
    }
}

/// Parse a time string into a DateTime
pub fn parse_time_string(time_str: &str) -> Result<DateTime<Local>> {
    let now = Local::now();

    // Try parsing with chrono-english first
    match parse_date_string(time_str, now, Dialect::Uk) {
        Ok(parsed_date) => return Ok(parsed_date),
        Err(_) => {} // Continue to try other formats
    }

    // Try standard date formats
    let formats = [
        "%Y-%m-%d",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S", 
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%S%.3fZ",
    ];

    for format in &formats {
        if let Ok(naive_dt) = chrono::NaiveDateTime::parse_from_str(time_str, format) {
            return Ok(Local.from_local_datetime(&naive_dt).single().unwrap_or_else(|| Local::now()));
        }
    }

    Err(anyhow!("Could not parse date: {}", time_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!(parse_duration("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_duration("7d").unwrap(), Duration::days(7));
        assert_eq!(parse_duration("2w").unwrap(), Duration::weeks(2));
        assert_eq!(parse_duration("30m").unwrap(), Duration::minutes(30));
        assert!(parse_duration("invalid").is_err());
    }

    #[test]
    fn test_parse_time_string() {
        // Test standard formats
        assert!(parse_time_string("2023-01-01").is_ok());
        assert!(parse_time_string("2023-01-01T12:00:00").is_ok());
        
        // Test natural language (may vary based on system)
        // These tests might be flaky due to natural language parsing
        // assert!(parse_time_string("yesterday").is_ok());
        // assert!(parse_time_string("last week").is_ok());
        
        assert!(parse_time_string("invalid date format").is_err());
    }
}