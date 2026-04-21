use anyhow::{Context, Result};
use buildinfo::version_string;
use clap::Parser;
use dialoguer::Confirm;
use std::time::Instant;

mod r2_client;
mod r2_wrangler;

use r2_wrangler::R2WranglerClient;

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

    // Create R2 wrangler client
    let client = R2WranglerClient::new();

    let mut total_deleted = 0;
    let mut pass = 0;
    let start_time = Instant::now();

    loop {
        pass += 1;

        if pass > 1 && !args.all {
            println!("\n📋 Pass {} - Checking for more objects...", pass);
        }

        // List objects in the bucket
        if pass == 1 {
            println!("Listing objects in bucket '{}'...", args.bucket);
        }

        let (keys, has_more) = client
            .list_objects(&args.bucket)
            .await
            .context("Failed to list objects")?;

        if keys.is_empty() {
            if pass == 1 {
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
            } else {
                println!(
                    "\n✅ All objects have been deleted from bucket '{}'",
                    args.bucket
                );
                println!("Total objects deleted: {}", total_deleted);
                println!("Total time: {:.2}s", start_time.elapsed().as_secs_f64());
                if args.delete_bucket {
                    client
                        .delete_bucket(&args.bucket)
                        .await
                        .context("Failed to delete bucket")?;
                    println!("✅ Bucket '{}' deleted.", args.bucket);
                }
            }
            return Ok(());
        }

        // Display objects
        if pass == 1 {
            // Show object count with special handling for --all flag
            if args.all && has_more {
                println!("\nFound at least 20 objects");
                if !args.list_only {
                    println!("⚠️  Warning: This will delete more files than shown. The tool will continue deleting until the bucket is empty.");
                }
            } else {
                println!("\nFound {} objects:", keys.len());
            }

            // Only show file list if NOT using --all (or if --list-only is set)
            if !args.all || args.list_only {
                if keys.len() <= 10 || args.list_only {
                    for key in &keys {
                        println!("  {}", key);
                    }
                } else {
                    // Show first 5 and last 5
                    for key in keys.iter().take(5) {
                        println!("  {}", key);
                    }
                    println!("  ... ({} more) ...", keys.len() - 10);
                    for key in keys.iter().skip(keys.len() - 5) {
                        println!("  {}", key);
                    }
                }
            }
        }

        if args.list_only {
            if has_more {
                println!(
                    "\n⚠️  Note: There are more objects in the bucket (showing first 20 only)"
                );
            }
            return Ok(());
        }

        // Handle pagination warning and automatic continuation
        if has_more && !args.all && pass == 1 {
            println!("\n⚠️  This bucket contains more than 20 objects.");
            println!("You can:");
            println!("  1. Run with --all flag to automatically delete all objects");
            println!("  2. Run the command multiple times manually");
            println!("  3. Use rclone for bulk operations: https://developers.cloudflare.com/r2/examples/rclone/");
        }

        // Ask for confirmation unless --force is used
        let proceed = if args.force {
            true
        } else if pass == 1 {
            let count_phrase = if has_more && !args.all {
                format!("these {}", keys.len())
            } else if has_more {
                "ALL".to_string()
            } else {
                keys.len().to_string()
            };
            println!("{}", warning_text(&count_phrase, args.delete_bucket));
            Confirm::new()
                .with_prompt("Do you want to proceed?")
                .default(false)
                .interact()
                .context("Failed to get user confirmation")?
        } else {
            true // Already confirmed on first pass
        };

        if !proceed {
            println!("Operation cancelled.");
            return Ok(());
        }

        // Delete objects
        client
            .delete_objects(&args.bucket, keys.clone())
            .await
            .context("Failed to delete objects")?;

        total_deleted += keys.len();

        // If not in --all mode or no more objects, stop
        if !args.all || !has_more {
            if pass == 1 {
                let duration = start_time.elapsed();
                println!(
                    "\n✅ Successfully deleted {} objects from bucket '{}' in {:.2}s",
                    total_deleted,
                    args.bucket,
                    duration.as_secs_f64()
                );
                if has_more {
                    println!("Note: There are more objects in the bucket. Run again to continue.");
                } else if args.delete_bucket {
                    client
                        .delete_bucket(&args.bucket)
                        .await
                        .context("Failed to delete bucket")?;
                    println!("✅ Bucket '{}' deleted.", args.bucket);
                }
            }
            break;
        }

        // Show progress for --all mode
        println!("✓ Pass {} complete: {} objects deleted", pass, keys.len());
        println!("Total deleted so far: {}", total_deleted);

        // Small delay between passes to avoid overwhelming the API
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
