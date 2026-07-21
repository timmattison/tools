use anyhow::{Context, Result};
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    models::{Page, Voucher, VoucherCreateRequest, VoucherCreateResponse, VoucherDeletionResults},
    output::{print_single_item, print_vec_table, OutputFormat},
    pagination::fetch_all_matching,
    prompt::{self, Approval, Console},
    site_helper::get_site_id_or_prompt,
};
use std::future::Future;

#[derive(Subcommand, Debug)]
pub enum VouchersCommand {
    /// List vouchers on a site
    List {
        /// Maximum number of vouchers to return
        #[clap(long, default_value = "100")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },

    /// Get voucher details
    Get {
        /// Voucher ID
        voucher_id: Uuid,
    },

    /// Create new vouchers
    Create {
        /// Number of vouchers to create
        #[clap(long, default_value = "1")]
        count: u32,

        /// Voucher name/note
        #[clap(long)]
        name: String,

        /// Time limit in minutes
        #[clap(long)]
        time_limit_minutes: u64,

        /// Maximum number of guests per voucher
        #[clap(long)]
        authorized_guest_limit: Option<u64>,

        /// Data usage limit in megabytes
        #[clap(long)]
        data_usage_limit_mbytes: Option<u64>,

        /// Download rate limit in kilobits per second
        #[clap(long)]
        rx_rate_limit_kbps: Option<u64>,

        /// Upload rate limit in kilobits per second
        #[clap(long)]
        tx_rate_limit_kbps: Option<u64>,
    },

    /// Delete a specific voucher
    Delete {
        /// Voucher ID
        voucher_id: Uuid,
    },

    /// Delete vouchers by filter
    DeleteFiltered {
        /// Filter expression
        #[clap(long)]
        filter: String,

        /// Delete without asking for confirmation
        #[clap(long, short = 'y')]
        yes: bool,

        /// List the vouchers the filter matches without deleting any of them
        #[clap(long)]
        dry_run: bool,
    },
}

#[derive(Tabled, serde::Serialize)]
struct VoucherRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Code")]
    code: String,
    #[tabled(rename = "Time Limit (min)")]
    time_limit_minutes: String,
    #[tabled(rename = "Guest Limit")]
    guest_limit: String,
    #[tabled(rename = "Guest Count")]
    guest_count: String,
    #[tabled(rename = "Expired")]
    expired: String,
    #[tabled(rename = "Created At")]
    created_at: String,
}

impl From<&Voucher> for VoucherRow {
    fn from(voucher: &Voucher) -> Self {
        Self {
            id: voucher.id.to_string(),
            name: voucher.name.clone(),
            code: voucher.code.clone(),
            time_limit_minutes: voucher.time_limit_minutes.to_string(),
            guest_limit: voucher
                .authorized_guest_limit
                .map(|l| l.to_string())
                .unwrap_or("unlimited".to_string()),
            guest_count: voucher.authorized_guest_count.to_string(),
            expired: voucher.expired.to_string(),
            created_at: voucher.created_at.clone(),
        }
    }
}

pub async fn handle_vouchers_command(
    command: VouchersCommand,
    site_id: Option<Uuid>,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        VouchersCommand::List {
            limit,
            offset,
            filter,
        } => list_vouchers(client, site_id, limit, offset, filter, output_format).await,
        VouchersCommand::Get { voucher_id } => {
            get_voucher(client, site_id, voucher_id, output_format).await
        }
        VouchersCommand::Create {
            count,
            name,
            time_limit_minutes,
            authorized_guest_limit,
            data_usage_limit_mbytes,
            rx_rate_limit_kbps,
            tx_rate_limit_kbps,
        } => {
            let request = VoucherCreateRequest {
                count,
                name,
                time_limit_minutes,
                authorized_guest_limit,
                data_usage_limit_mbytes,
                rx_rate_limit_kbps,
                tx_rate_limit_kbps,
            };
            create_vouchers(client, site_id, request, output_format).await
        }
        VouchersCommand::Delete { voucher_id } => delete_voucher(client, site_id, voucher_id).await,
        VouchersCommand::DeleteFiltered {
            filter,
            yes,
            dry_run,
        } => {
            let options = DeleteOptions {
                assume_yes: yes,
                dry_run,
            };
            delete_vouchers_filtered(client, site_id, filter, options, output_format).await
        }
    }
}

async fn list_vouchers(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    limit: u32,
    offset: u64,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let limit_str = limit.to_string();
    let offset_str = offset.to_string();
    let mut params: Vec<(&str, &dyn std::fmt::Display)> =
        vec![("limit", &limit_str), ("offset", &offset_str)];

    if let Some(f) = &filter {
        params.push(("filter", f));
    }

    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);
    let page: Page<Voucher> = client.get_with_params(&path, &params).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&page, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<VoucherRow> = page.data.iter().map(VoucherRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

async fn get_voucher(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    voucher_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers/{}", site_id, voucher_id);
    let voucher: Voucher = client.get(&path).await?;

    print_single_item(&voucher, output_format)?;
    Ok(())
}

async fn create_vouchers(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    request: VoucherCreateRequest,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);

    let response: VoucherCreateResponse = client.post(&path, &request).await?;

    match output_format {
        OutputFormat::Json => {
            print_single_item(&response.vouchers, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<VoucherRow> = response.vouchers.iter().map(VoucherRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

async fn delete_voucher(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    voucher_id: Uuid,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers/{}", site_id, voucher_id);
    let result: VoucherDeletionResults = client.delete(&path).await?;

    println!("Deleted {} voucher(s)", result.vouchers_deleted);
    Ok(())
}

/// How much a filtered deletion is allowed to do on its own.
#[derive(Debug, Clone, Copy, Default)]
struct DeleteOptions {
    /// The user passed `--yes`: do not stop to ask.
    assume_yes: bool,
    /// The user passed `--dry-run`: list the matches, delete nothing.
    dry_run: bool,
}

/// What a filtered deletion ended up doing.
#[derive(Debug, PartialEq, Eq)]
enum DeletionOutcome {
    /// The filter matched no vouchers.
    NoMatches,
    /// `--dry-run`: the matches were listed and nothing was deleted.
    Listed,
    /// The deletion was approved and this many vouchers were destroyed.
    Deleted(u64),
    /// The user was asked and declined.
    Aborted,
}

/// Run `delete` only once the user has agreed to lose `match_count` vouchers.
///
/// The deletion is passed in as a closure so the decision — ask, refuse,
/// abort, delete — can be exercised without a controller, and so a test can
/// prove that a declined confirmation never reaches the API at all.
///
/// # Arguments
///
/// * `match_count` - How many vouchers the filter matched.
/// * `options` - The `--yes` / `--dry-run` flags.
/// * `console` - Where the confirmation is put to the user.
/// * `delete` - Performs the deletion, answering with the number destroyed.
///
/// # Errors
///
/// Returns an error if the confirmation cannot be obtained (a non-terminal
/// stdin without `--yes`) or if the deletion itself fails.
async fn confirm_then_delete<D, Fut>(
    match_count: usize,
    options: DeleteOptions,
    console: &mut impl Console,
    delete: D,
) -> Result<DeletionOutcome>
where
    D: FnOnce() -> Fut,
    Fut: Future<Output = Result<u64>>,
{
    let question = format!("Delete {match_count} voucher(s)?");

    // A dry run is a confirmation that has already been answered "no": the
    // matches have been listed, so there is nothing left to ask and nothing
    // to destroy.
    let approval = prompt::confirm_destructive(
        console,
        &question,
        match_count,
        options.assume_yes || options.dry_run,
    )?;

    match approval {
        Approval::NothingMatched => Ok(DeletionOutcome::NoMatches),
        Approval::Declined => Ok(DeletionOutcome::Aborted),
        Approval::Approved if options.dry_run => Ok(DeletionOutcome::Listed),
        Approval::Approved => Ok(DeletionOutcome::Deleted(delete().await?)),
    }
}

/// Delete every voucher a filter expression matches.
///
/// The matches are listed first and then confirmed, because the filter is
/// evaluated by the controller: the only way to know what a filter really
/// selects is to look at what came back, and by the time the API has answered
/// a `DELETE` it is too late.
async fn delete_vouchers_filtered(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    filter: String,
    options: DeleteOptions,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);

    let matches: Vec<Voucher> = fetch_all_matching(client, &path, Some(&filter))
        .await
        .context("Failed to list the vouchers the filter matches")?;

    if !matches.is_empty() {
        let rows: Vec<VoucherRow> = matches.iter().map(VoucherRow::from).collect();
        print_vec_table(&rows, output_format)?;
    }

    let params: Vec<(&str, &dyn std::fmt::Display)> = vec![("filter", &filter)];
    let outcome = confirm_then_delete(matches.len(), options, &mut prompt::Stdio, || async {
        let result: VoucherDeletionResults = client.delete_with_params(&path, &params).await?;
        Ok(result.vouchers_deleted)
    })
    .await?;

    match outcome {
        DeletionOutcome::NoMatches => {
            println!("No vouchers match that filter; nothing to delete.");
        }
        DeletionOutcome::Listed => {
            println!(
                "Dry run: {} voucher(s) would be deleted. Re-run without --dry-run to delete them.",
                matches.len()
            );
        }
        DeletionOutcome::Aborted => println!("Aborted; no vouchers were deleted."),
        DeletionOutcome::Deleted(deleted) => println!("Deleted {deleted} voucher(s)"),
    }

    Ok(())
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    use crate::prompt::Scripted;
    use std::cell::Cell;

    /// How many vouchers the pretend controller holds.
    const MATCHES: usize = 7;

    /// A deletion that records whether it was ever reached.
    fn recording_delete(
        reached: &Cell<bool>,
    ) -> impl FnOnce() -> std::future::Ready<Result<u64>> + '_ {
        move || {
            reached.set(true);
            std::future::ready(Ok(u64::try_from(MATCHES).unwrap()))
        }
    }

    /// `--yes` is how a script says "I already know": delete, ask nothing.
    #[tokio::test]
    async fn assume_yes_deletes_without_asking() {
        let reached = Cell::new(false);
        let mut console = Scripted::terminal(&[]);

        let outcome = confirm_then_delete(
            MATCHES,
            DeleteOptions {
                assume_yes: true,
                dry_run: false,
            },
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect("--yes must be able to delete");

        assert_eq!(
            outcome,
            DeletionOutcome::Deleted(u64::try_from(MATCHES).unwrap())
        );
        assert!(reached.get(), "--yes must actually delete");
        assert!(!console.was_asked(), "--yes must not stop to ask");
    }

    /// Saying no must leave every voucher alone.
    #[tokio::test]
    async fn a_declined_confirmation_deletes_nothing() {
        let reached = Cell::new(false);
        let mut console = Scripted::terminal(&["n"]);

        let outcome = confirm_then_delete(
            MATCHES,
            DeleteOptions::default(),
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect("declining is not an error");

        assert_eq!(outcome, DeletionOutcome::Aborted);
        assert!(
            !reached.get(),
            "a declined deletion must never reach the API"
        );
        assert!(console.was_asked(), "the user must have been asked");
    }

    /// Hitting return at a `[y/N]` prompt is a no, not a yes.
    #[tokio::test]
    async fn an_empty_answer_deletes_nothing() {
        let reached = Cell::new(false);
        let mut console = Scripted::terminal(&["\n"]);

        let outcome = confirm_then_delete(
            MATCHES,
            DeleteOptions::default(),
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect("declining is not an error");

        assert_eq!(outcome, DeletionOutcome::Aborted);
        assert!(!reached.get(), "an empty answer must not destroy anything");
    }

    /// Piped into, with no `--yes`, there is nobody to confirm: refuse.
    #[tokio::test]
    async fn a_pipe_without_yes_refuses_to_delete() {
        let reached = Cell::new(false);
        let mut console = Scripted::not_a_terminal();

        let error = confirm_then_delete(
            MATCHES,
            DeleteOptions::default(),
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect_err("a non-interactive bulk deletion must refuse");

        assert!(
            format!("{error:#}").contains("--yes"),
            "the refusal must say how to proceed, got {error:#}"
        );
        assert!(!reached.get(), "a refused deletion must not reach the API");
    }

    /// A filter that matched nothing is not worth a question.
    #[tokio::test]
    async fn zero_matches_asks_nothing_and_deletes_nothing() {
        let reached = Cell::new(false);
        let mut console = Scripted::terminal(&[]);

        let outcome = confirm_then_delete(
            0,
            DeleteOptions::default(),
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect("an empty match is not an error");

        assert_eq!(outcome, DeletionOutcome::NoMatches);
        assert!(!reached.get(), "there is nothing to delete");
        assert!(!console.was_asked(), "there is nothing to ask about");
    }

    /// `--dry-run` is how you find out what a filter matches, safely.
    #[tokio::test]
    async fn dry_run_lists_without_deleting() {
        let reached = Cell::new(false);
        let mut console = Scripted::terminal(&[]);

        let outcome = confirm_then_delete(
            MATCHES,
            DeleteOptions {
                assume_yes: false,
                dry_run: true,
            },
            &mut console,
            recording_delete(&reached),
        )
        .await
        .expect("a dry run is not an error");

        assert_eq!(outcome, DeletionOutcome::Listed);
        assert!(!reached.get(), "a dry run must not delete anything");
        assert!(!console.was_asked(), "a dry run has nothing to confirm");
    }
}
