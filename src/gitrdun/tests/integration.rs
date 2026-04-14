use anyhow::Result;
use chrono::{Duration, Local};
use gitrdun::{cli::Args, date, git::SearchResult, results::format_results, stats::GitStats};
use tokio_util::sync::CancellationToken;

#[test]
fn test_parse_duration() -> Result<()> {
    // Test basic formats
    assert_eq!(date::parse_duration("24h")?, Duration::hours(24));
    assert_eq!(date::parse_duration("7d")?, Duration::days(7));
    assert_eq!(date::parse_duration("2w")?, Duration::weeks(2));
    assert_eq!(date::parse_duration("30m")?, Duration::minutes(30));

    // Test invalid formats
    assert!(date::parse_duration("invalid").is_err());

    Ok(())
}

#[test]
fn test_parse_time_string() -> Result<()> {
    // Test standard formats
    assert!(date::parse_time_string("2023-01-01").is_ok());
    assert!(date::parse_time_string("2023-01-01T12:00:00").is_ok());

    // Test invalid format
    assert!(date::parse_time_string("invalid date format").is_err());

    Ok(())
}

#[test]
fn test_git_stats() {
    let stats = GitStats::new();

    // Test recording operations
    stats.record_git_dir(std::time::Duration::from_millis(100));
    stats.record_log(std::time::Duration::from_millis(200));
    stats.record_email(std::time::Duration::from_millis(50));

    // Test that we can lock and access the stats
    if let Ok(git_dir_stats) = stats.get_git_dir.lock() {
        assert_eq!(git_dir_stats.count(), 1);
    }

    if let Ok(log_stats) = stats.get_log.lock() {
        assert_eq!(log_stats.count(), 1);
    }

    if let Ok(email_stats) = stats.get_email.lock() {
        assert_eq!(email_stats.count(), 1);
    };
}

/// Regression test for the duplicate-results-files bug.
///
/// Previously, `main()` called `display_results` twice (once while the TUI
/// was running and once after it exited). Each call wrote a timestamped
/// output file, producing two files ~1 second apart with identical
/// content.
///
/// The fix separates formatting (pure, returns a String) from file
/// writing (side effect). `format_results` must be a pure function with
/// no file-system side effects so the caller can control when/how many
/// times the output is written to disk.
#[tokio::test]
async fn test_format_results_is_pure_and_returns_formatted_output() -> Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let args = Args {
        // Route auto-file writing into the temp dir (if any side effect
        // occurs, we'll detect it here).
        output: Some(temp_dir.path().join("should-not-exist.txt")),
        no_file: false,
        ..Args::default()
    };
    let result = SearchResult::new(Local::now(), None);

    let output = format_results(&result, &args, false, None, CancellationToken::new()).await?;

    assert!(
        output.contains("No commits"),
        "Expected formatted output to mention no commits, got: {output:?}"
    );

    let leftover_files: Vec<_> = std::fs::read_dir(temp_dir.path())?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .collect();
    assert!(
        leftover_files.is_empty(),
        "format_results must not write files; found: {leftover_files:?}"
    );

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use clap::Parser;
    use gitrdun::cli::Args;
    use std::path::PathBuf;

    #[test]
    fn test_cli_defaults() {
        let args = Args::parse_from(&["gitrdun"]);

        assert_eq!(args.start, "24h");
        assert_eq!(args.end, None);
        assert!(!args.ignore_failures);
        assert!(!args.summary_only);
        assert!(!args.find_nested);
        assert!(!args.stats);
        assert!(!args.all);
        assert!(!args.ollama);
        assert!(!args.meta_ollama);
        assert_eq!(args.ollama_model, "gpt-oss");
        assert_eq!(args.ollama_url, "http://localhost:11434");
        assert!(args.filter_user);
        assert!(!args.keep_thinking);
    }

    #[test]
    fn test_cli_with_args() {
        let args = Args::parse_from(&[
            "gitrdun",
            "--start",
            "7d",
            "--end",
            "2023-12-31",
            "--ignore-failures",
            "--summary-only",
            "--find-nested",
            "--stats",
            "--all",
            "--ollama",
            "--meta-ollama",
            "--ollama-model",
            "llama2:7b",
            "--ollama-url",
            "http://example.com:11434",
            "--output",
            "/tmp/output.txt",
            "--keep-thinking",
            "path1",
            "path2",
        ]);

        assert_eq!(args.start, "7d");
        assert_eq!(args.end, Some("2023-12-31".to_string()));
        assert!(args.ignore_failures);
        assert!(args.summary_only);
        assert!(args.find_nested);
        assert!(args.stats);
        assert!(args.all);
        assert!(args.ollama);
        assert!(args.meta_ollama);
        assert_eq!(args.ollama_model, "llama2:7b");
        assert_eq!(args.ollama_url, "http://example.com:11434");
        assert_eq!(args.output, Some(PathBuf::from("/tmp/output.txt")));
        assert!(args.keep_thinking);
        assert_eq!(
            args.paths,
            vec![PathBuf::from("path1"), PathBuf::from("path2")]
        );
    }
}
