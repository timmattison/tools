use anyhow::{Context, Result};
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    models::{Page, Voucher, VoucherCreateRequest, VoucherCreateResponse, VoucherDeletionResults},
    output::{
        print_output, print_vec_table, render_collection, render_page_listing, OutputFormat, Report,
    },
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
        } => list_vouchers(client, site_id, limit, offset, filter, output_format)
            .await
            .map(Report::print),
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
        VouchersCommand::Delete { voucher_id } => delete_voucher(client, site_id, voucher_id)
            .await
            .map(Report::print),
        VouchersCommand::DeleteFiltered {
            filter,
            yes,
            dry_run,
        } => {
            let options = DeleteOptions {
                assume_yes: yes,
                dry_run,
            };
            delete_vouchers_filtered(
                client,
                site_id,
                filter,
                options,
                output_format,
                &mut prompt::Stdio,
                Report::print,
            )
            .await
        }
    }
}

/// List one page of the hotspot vouchers of a site.
///
/// # Arguments
///
/// * `client` - The controller client to list the vouchers with.
/// * `site_id` - The site the user named, if any.
/// * `limit` - How many vouchers to ask for.
/// * `offset` - Where in the collection the page starts.
/// * `filter` - The API's filter expression, if the user gave one.
/// * `output_format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered page, ready to print.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, if the request fails, or
/// if the answer cannot be rendered.
async fn list_vouchers(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    limit: u32,
    offset: u64,
    filter: Option<String>,
    output_format: OutputFormat,
) -> Result<Report> {
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

    render_page_listing::<Voucher, VoucherRow>(&page, output_format)
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

    print_output(&voucher, output_format)?;
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
            print_output(&response.vouchers, output_format)?;
        }
        OutputFormat::Table => {
            let rows: Vec<VoucherRow> = response.vouchers.iter().map(VoucherRow::from).collect();
            print_vec_table(&rows, output_format)?;
        }
    }

    Ok(())
}

/// Delete one voucher by its id.
///
/// # Arguments
///
/// * `client` - The controller client to send the request with.
/// * `site_id` - The site the user named, if any.
/// * `voucher_id` - The voucher to delete.
///
/// # Returns
///
/// The report that says how many vouchers the controller deleted.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, or if the request fails.
async fn delete_voucher(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    voucher_id: Uuid,
) -> Result<Report> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers/{}", site_id, voucher_id);
    let result: VoucherDeletionResults = client.delete(&path).await?;

    Ok(Report::of_note(format!(
        "Deleted {} voucher(s)",
        result.vouchers_deleted
    )))
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
///
/// The command shows two reports, and the question comes between them: the
/// matches, and then what the deletion did. `show` receives each report when
/// the user reads it, so a test sees both without a capture of the process
/// streams.
///
/// # Arguments
///
/// * `client` - The controller client to send the requests with.
/// * `site_id` - The site the user named, if any.
/// * `filter` - The API's filter expression.
/// * `options` - The `--yes` / `--dry-run` flags.
/// * `output_format` - The output format the user asked for.
/// * `console` - Where the confirmation is put to the user.
/// * `show` - Receives each report, in the order the user reads them.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, if a request fails, if
/// the confirmation cannot be obtained, or if the matches cannot be rendered.
async fn delete_vouchers_filtered(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    filter: String,
    options: DeleteOptions,
    output_format: OutputFormat,
    console: &mut impl Console,
    mut show: impl FnMut(Report),
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/hotspot/vouchers", site_id);

    let matches: Vec<Voucher> = fetch_all_matching(client, &path, Some(&filter))
        .await
        .context("Failed to list the vouchers the filter matches")?;

    let rows: Vec<VoucherRow> = matches.iter().map(VoucherRow::from).collect();
    show(render_collection(&rows, output_format)?);

    let params: Vec<(&str, &dyn std::fmt::Display)> = vec![("filter", &filter)];
    let outcome = confirm_then_delete(matches.len(), options, console, || async {
        let result: VoucherDeletionResults = client.delete_with_params(&path, &params).await?;
        Ok(result.vouchers_deleted)
    })
    .await?;

    show(outcome_report(outcome, matches.len()));

    Ok(())
}

/// The report that says what a filtered deletion did.
///
/// # Arguments
///
/// * `outcome` - What the deletion did.
/// * `match_count` - How many vouchers the filter matched.
///
/// # Returns
///
/// The report of the outcome.
fn outcome_report(outcome: DeletionOutcome, match_count: usize) -> Report {
    let sentence = match outcome {
        DeletionOutcome::NoMatches => "No vouchers match that filter; nothing to delete.".to_string(),
        DeletionOutcome::Listed => format!(
            "Dry run: {match_count} voucher(s) would be deleted. Re-run without --dry-run to delete them."
        ),
        DeletionOutcome::Aborted => "Aborted; no vouchers were deleted.".to_string(),
        DeletionOutcome::Deleted(deleted) => format!("Deleted {deleted} voucher(s)"),
    };

    Report::of_note(sentence)
}

#[cfg(test)]
mod deletion_tests {
    use super::*;
    use crate::prompt::Scripted;
    use crate::test_server::{json_response, TestServer};
    use std::cell::Cell;

    /// How many vouchers the pretend controller holds.
    const MATCHES: usize = 7;

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// What the controller answers a single deletion with.
    const ONE_DELETED: &str = r#"{"vouchersDeleted":1}"#;

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

    /// A voucher named by id is deleted by a `DELETE` on that voucher's own
    /// path, under the site the user named.
    ///
    /// The assertion reads the request the *server* got rather than the URL
    /// the client built, because the path is what decides which voucher the
    /// controller destroys. A deletion that reached the site path, or that
    /// carried the site id where the voucher id belongs, would take vouchers
    /// the user never named.
    #[tokio::test]
    async fn deleting_one_voucher_sends_a_delete_for_the_id_it_was_given() {
        let controller = TestServer::replying(&json_response(ONE_DELETED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let site_id = Uuid::new_v4();
        let voucher_id = Uuid::new_v4();

        let _report = delete_voucher(&client, Some(site_id), voucher_id)
            .await
            .expect("the controller answered the deletion");

        let received = controller.requests();
        assert_eq!(
            received.len(),
            1,
            "a named voucher costs exactly one request, got {received:?}"
        );
        assert_eq!(
            received[0].request_line(),
            format!(
                "DELETE /proxy/network/integration/v1/sites/{site_id}/hotspot/vouchers/{voucher_id} HTTP/1.1"
            ),
            "the deletion must name the voucher the user named, got {received:?}"
        );
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

    /// A voucher deleted by id has no document to give. The count the
    /// controller answered with is a note, so it goes to standard error.
    #[tokio::test]
    async fn deleting_one_voucher_reports_on_standard_error_only() {
        let controller = TestServer::replying(&json_response(ONE_DELETED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let report = delete_voucher(&client, Some(Uuid::new_v4()), Uuid::new_v4())
            .await
            .expect("the controller answered the deletion");

        assert_eq!(
            report.document(),
            None,
            "a deletion by id must put nothing on standard output"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("Deleted 1 voucher(s)")),
            "the note must say how many vouchers the controller deleted, got {:?}",
            report.notes()
        );
    }

    /// Every outcome of a filtered deletion is a sentence for a person. Each
    /// one is a note, and none of them is part of the document, so the list
    /// of matches under `--output json` stays one document.
    #[test]
    fn every_deletion_outcome_is_a_note_and_not_part_of_the_document() {
        let deleted = u64::try_from(MATCHES).unwrap();

        for (outcome, sentence) in [
            (DeletionOutcome::NoMatches, "No vouchers match that filter"),
            (
                DeletionOutcome::Listed,
                "Dry run: 7 voucher(s) would be deleted",
            ),
            (
                DeletionOutcome::Aborted,
                "Aborted; no vouchers were deleted",
            ),
            (DeletionOutcome::Deleted(deleted), "Deleted 7 voucher(s)"),
        ] {
            let label = format!("{outcome:?}");
            let report = outcome_report(outcome, MATCHES);

            assert_eq!(
                report.document(),
                None,
                "the {label} outcome must put nothing on standard output"
            );
            assert!(
                report.notes().iter().any(|note| note.contains(sentence)),
                "the {label} outcome must say {sentence:?}, got {:?}",
                report.notes()
            );
        }
    }

    /// One voucher, and the whole of what the filter matches.
    const ONE_MATCHING_VOUCHER: &str = r#"{
        "offset": 0, "limit": 200, "count": 1, "totalCount": 1,
        "data": [
            {
                "id": "00000000-0000-0000-0000-000000000005",
                "createdAt": "2026-07-21T00:00:00Z",
                "name": "lobby",
                "code": "1234567890",
                "authorizedGuestCount": 0,
                "expired": true,
                "timeLimitMinutes": 60
            }
        ]
    }"#;

    /// A filter that matches no voucher at all.
    const NO_MATCHING_VOUCHERS: &str =
        r#"{ "offset": 0, "limit": 200, "count": 0, "totalCount": 0, "data": [] }"#;

    /// The filter a test hands the command. The pretend controller does not
    /// read it.
    const A_FILTER: &str = "expired.eq(true)";

    /// Run `delete-filtered` against a controller that answers every request
    /// with `page`.
    ///
    /// # Arguments
    ///
    /// * `page` - The JSON document the controller answers with.
    /// * `options` - The `--yes` / `--dry-run` flags.
    /// * `format` - The output format the user asked for.
    ///
    /// # Returns
    ///
    /// Every report the command showed, in the order the user reads them.
    async fn run_delete_filtered(
        page: &str,
        options: DeleteOptions,
        format: OutputFormat,
    ) -> Vec<Report> {
        let controller = TestServer::replying(&json_response(page)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");
        let mut console = Scripted::terminal(&[]);
        let mut reports = Vec::new();

        delete_vouchers_filtered(
            &client,
            Some(Uuid::new_v4()),
            A_FILTER.to_string(),
            options,
            format,
            &mut console,
            |report| reports.push(report),
        )
        .await
        .expect("the controller answered every request");

        reports
    }

    /// What a run writes on each stream.
    ///
    /// # Arguments
    ///
    /// * `reports` - Every report the run showed.
    ///
    /// # Returns
    ///
    /// Standard output, then standard error, one line for each document and
    /// each note.
    fn streams(reports: &[Report]) -> (String, String) {
        let stdout: Vec<&str> = reports.iter().filter_map(Report::document).collect();
        let stderr: Vec<&str> = reports
            .iter()
            .flat_map(|report| report.notes().iter().map(String::as_str))
            .collect();

        (stdout.join("\n"), stderr.join("\n"))
    }

    /// The finding this answers: `delete-filtered --dry-run --output json`
    /// printed the matches and then a sentence, both on standard output, so
    /// no program could parse what it wrote.
    #[tokio::test]
    async fn a_json_dry_run_writes_only_the_matches_on_standard_output() {
        let reports = run_delete_filtered(
            ONE_MATCHING_VOUCHER,
            DeleteOptions {
                assume_yes: false,
                dry_run: true,
            },
            OutputFormat::Json,
        )
        .await;
        let (stdout, stderr) = streams(&reports);

        let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|error| {
            panic!("--output json must write one JSON document ({error}), got:\n{stdout}")
        });
        assert_eq!(
            parsed.as_array().map(Vec::len),
            Some(1),
            "the document must list the one match, got:\n{stdout}"
        );
        assert!(
            stderr.contains("Dry run"),
            "the dry run must say so on standard error, got:\n{stderr}"
        );
    }

    /// A filter that matches nothing still answers `--output json` with a
    /// document: `[]`. Before, the run wrote only a sentence.
    #[tokio::test]
    async fn a_json_filter_that_matches_nothing_answers_an_empty_array() {
        let reports = run_delete_filtered(
            NO_MATCHING_VOUCHERS,
            DeleteOptions::default(),
            OutputFormat::Json,
        )
        .await;
        let (stdout, stderr) = streams(&reports);

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stdout).ok(),
            Some(serde_json::json!([])),
            "a filter that matches nothing must answer with an empty array, got:\n{stdout}"
        );
        assert!(
            stderr.contains("No vouchers match"),
            "the run must say that nothing matched, on standard error, got:\n{stderr}"
        );
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::test_server::{json_response, TestServer};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// How many vouchers the pretend controller says it holds.
    const TOTAL: &str = "100";

    /// One page of the voucher collection: a single item, out of a hundred.
    const ONE_VOUCHER_OF_A_HUNDRED: &str = r#"{
        "offset": 0, "limit": 1, "count": 1, "totalCount": 100,
        "data": [
            {
                "id": "00000000-0000-0000-0000-000000000005",
                "createdAt": "2026-07-21T00:00:00Z",
                "name": "lobby",
                "code": "1234567890",
                "authorizedGuestCount": 0,
                "expired": false,
                "timeLimitMinutes": 60
            }
        ]
    }"#;

    /// A page that holds a single voucher of a hundred looks exactly like the whole
    /// listing of a controller that has one. The note that tells the two apart
    /// is decided in `output`, and this proves the voucher listing asks for it: a
    /// command that renders its page on its own answers with no note at all.
    #[tokio::test]
    async fn a_short_page_of_vouchers_reports_what_it_left_out() {
        let controller = TestServer::replying(&json_response(ONE_VOUCHER_OF_A_HUNDRED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let listing = list_vouchers(
            &client,
            Some(Uuid::new_v4()),
            1,
            0,
            None,
            OutputFormat::Table,
        )
        .await
        .expect("the controller answered the listing");

        let notice = listing
            .notes()
            .first()
            .expect("a page short of the stated total must say so");
        assert!(
            notice.contains(TOTAL),
            "the note must report the total the controller stated, got: {notice}"
        );
        assert!(
            notice.contains("--limit") && notice.contains("--offset"),
            "the note must say how to reach the rest, got: {notice}"
        );
    }
}
