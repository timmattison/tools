use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use dialoguer::Confirm;
use indicatif::{ProgressBar, ProgressStyle};
use std::time::{Duration, Instant};
use termbar::{calculate_bar_width, TerminalWidth, PROGRESS_CHARS};

mod r2_client;

use r2_client::R2Client;

const DELETE_BAR_OVERHEAD: u16 = 60;

fn build_delete_progress_bar(total: u64) -> ProgressBar {
    let terminal_width = TerminalWidth::get_or_default();
    let bar_width = calculate_bar_width(terminal_width, DELETE_BAR_OVERHEAD);
    let template = format!(
        "Deleting [{{elapsed_precise}}] [{{bar:{bar_width}.cyan/blue}}] {{human_pos}}/{{human_len}} ({{per_sec}}, ~{{eta}})"
    );
    let style = ProgressStyle::default_bar()
        .template(&template)
        .expect("valid progress template")
        .progress_chars(PROGRESS_CHARS);
    let pb = ProgressBar::new(total);
    pb.set_style(style);
    pb
}

fn build_discovery_spinner() -> ProgressBar {
    let style = ProgressStyle::default_spinner()
        .template("{spinner:.cyan} Listing objects: {msg}")
        .expect("valid spinner template");
    let pb = ProgressBar::new_spinner();
    pb.set_style(style);
    pb.enable_steady_tick(Duration::from_millis(100));
    pb
}

fn warning_text(count_phrase: &str, delete_bucket: bool) -> String {
    if delete_bucket {
        format!(
            "\n⚠️  WARNING: This will permanently delete {} objects and delete the bucket itself!",
            count_phrase
        )
    } else {
        format!(
            "\n⚠️  WARNING: This will permanently delete {} objects!",
            count_phrase
        )
    }
}

#[derive(Parser, Debug)]
#[command(author, version = version_string!(), about, long_about = None)]
struct Args {
    /// Name of the R2 bucket
    bucket: String,

    /// Skip confirmation prompt and delete all objects
    #[arg(short, long)]
    force: bool,

    /// Only list objects, don't delete
    #[arg(short, long)]
    list_only: bool,

    /// Automatically continue until all objects are deleted (bypass 20 object limit)
    #[arg(short, long)]
    all: bool,

    /// After emptying, delete the bucket itself
    #[arg(short = 'd', long, conflicts_with = "list_only")]
    delete_bucket: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let client = R2Client::new()
        .await
        .context("Failed to initialize R2 client")?;

    let start_time = Instant::now();

    println!("Listing objects in bucket '{}'...", args.bucket);

    let (preview_keys, has_more) = client
        .list_objects(&args.bucket)
        .await
        .context("Failed to list objects")?;

    if preview_keys.is_empty() {
        println!("No objects found in bucket '{}'", args.bucket);
        if args.delete_bucket {
            let proceed = if args.force {
                true
            } else {
                Confirm::new()
                    .with_prompt(format!("Delete bucket '{}'?", args.bucket))
                    .default(false)
                    .interact()
                    .context("Failed to get user confirmation")?
            };
            if proceed {
                client
                    .delete_bucket(&args.bucket)
                    .await
                    .context("Failed to delete bucket")?;
                println!("✅ Bucket '{}' deleted.", args.bucket);
            } else {
                println!("Operation cancelled.");
            }
        }
        return Ok(());
    }

    if args.all && has_more {
        println!("\nFound at least 20 objects");
        if !args.list_only {
            println!("⚠️  Warning: This will delete more files than shown. The tool will continue deleting until the bucket is empty.");
        }
    } else {
        println!("\nFound {} objects:", preview_keys.len());
    }

    if !args.all || args.list_only {
        if preview_keys.len() <= 10 || args.list_only {
            for key in &preview_keys {
                println!("  {}", key);
            }
        } else {
            for key in preview_keys.iter().take(5) {
                println!("  {}", key);
            }
            println!("  ... ({} more) ...", preview_keys.len() - 10);
            for key in preview_keys.iter().skip(preview_keys.len() - 5) {
                println!("  {}", key);
            }
        }
    }

    if args.list_only {
        if has_more {
            println!("\n⚠️  Note: There are more objects in the bucket (showing first 20 only)");
        }
        return Ok(());
    }

    if has_more && !args.all {
        println!("\n⚠️  This bucket contains more than 20 objects.");
        println!("You can:");
        println!("  1. Run with --all flag to automatically delete all objects");
        println!("  2. Run the command multiple times manually");
        println!("  3. Use rclone for bulk operations: https://developers.cloudflare.com/r2/examples/rclone/");
    }

    let proceed = if args.force {
        true
    } else {
        let count_phrase = if has_more && !args.all {
            format!("these {}", preview_keys.len())
        } else if has_more {
            "ALL".to_string()
        } else {
            preview_keys.len().to_string()
        };
        println!("{}", warning_text(&count_phrase, args.delete_bucket));
        Confirm::new()
            .with_prompt("Do you want to proceed?")
            .default(false)
            .interact()
            .context("Failed to get user confirmation")?
    };

    if !proceed {
        println!("Operation cancelled.");
        return Ok(());
    }

    let keys_to_delete: Vec<String> = if args.all && has_more {
        let spinner = build_discovery_spinner();
        let keys = client
            .list_all_objects(&args.bucket, |count| {
                spinner.set_message(format!("{count} found"));
            })
            .await
            .context("Failed to list objects")?;
        spinner.finish_with_message(format!("{} objects to delete", keys.len()));
        keys
    } else {
        preview_keys
    };

    let total = keys_to_delete.len() as u64;
    let pb = build_delete_progress_bar(total);
    client
        .delete_objects(&args.bucket, &keys_to_delete, |n| {
            pb.inc(n as u64);
        })
        .await
        .context("Failed to delete objects")?;
    pb.finish();

    let duration = start_time.elapsed();
    println!(
        "\n✅ Successfully deleted {} objects from bucket '{}' in {:.2}s",
        keys_to_delete.len(),
        args.bucket,
        duration.as_secs_f64()
    );

    if args.delete_bucket {
        client
            .delete_bucket(&args.bucket)
            .await
            .context("Failed to delete bucket")?;
        println!("✅ Bucket '{}' deleted.", args.bucket);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    #[test]
    fn args_delete_bucket_long_parses() {
        let result = Args::try_parse_from(["r2-bucket-cleaner", "b", "--delete-bucket"]);
        assert!(
            result.is_ok(),
            "expected --delete-bucket to parse, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn args_delete_bucket_short_parses() {
        let result = Args::try_parse_from(["r2-bucket-cleaner", "b", "-d"]);
        assert!(
            result.is_ok(),
            "expected -d to parse, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn args_list_only_and_delete_bucket_conflict() {
        let result =
            Args::try_parse_from(["r2-bucket-cleaner", "b", "--list-only", "--delete-bucket"]);
        let err = result.expect_err("expected --list-only --delete-bucket to conflict");
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn args_delete_bucket_long_sets_flag() {
        let args = Args::try_parse_from(["r2-bucket-cleaner", "b", "--delete-bucket"]).unwrap();
        assert!(args.delete_bucket);
    }

    #[test]
    fn args_delete_bucket_short_sets_flag() {
        let args = Args::try_parse_from(["r2-bucket-cleaner", "b", "-d"]).unwrap();
        assert!(args.delete_bucket);
    }

    #[test]
    fn args_delete_bucket_defaults_false() {
        let args = Args::try_parse_from(["r2-bucket-cleaner", "b"]).unwrap();
        assert!(!args.delete_bucket);
    }

    #[test]
    fn warning_text_without_delete_bucket_mentions_only_objects() {
        let text = warning_text("5", false);
        assert!(
            text.contains("permanently delete 5 objects"),
            "got: {}",
            text
        );
        assert!(
            !text.to_lowercase().contains("bucket itself"),
            "got: {}",
            text
        );
    }

    #[test]
    fn warning_text_with_delete_bucket_mentions_bucket() {
        let text = warning_text("5", true);
        assert!(
            text.contains("permanently delete 5 objects"),
            "got: {}",
            text
        );
        assert!(
            text.contains("and delete the bucket itself"),
            "got: {}",
            text
        );
    }

    #[test]
    fn warning_text_preserves_count_phrase() {
        assert!(warning_text("these 20", false).contains("these 20 objects"));
        assert!(warning_text("ALL", true).contains("ALL objects"));
    }
}
