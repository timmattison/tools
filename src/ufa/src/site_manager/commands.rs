use crate::output::{render_output, render_vec_table, OutputFormat, Report};
use crate::site_manager::models::Host;
use crate::site_manager::utils::CLOUD_HOST_ID_DISPLAY_CHARS;
use crate::site_manager::SiteManagerClient;
use crate::text::truncate_for_display;
use anyhow::Result;
use clap::Subcommand;

/// Shown in place of a detail the console never reported.
const UNKNOWN: &str = "Unknown";

/// Shown in place of a value the API left out entirely.
const NO_DATA: &str = "N/A";

/// How a host's last state change is spelled in the listing.
const LAST_SEEN_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// One row of the cloud host listing.
#[derive(tabled::Tabled, serde::Serialize)]
pub struct HostRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Model")]
    model: String,
    #[tabled(rename = "Firmware")]
    firmware: String,
    #[tabled(rename = "IP Address")]
    ip_address: String,
    #[tabled(rename = "Type")]
    host_type: String,
    #[tabled(rename = "Owner")]
    owner: String,
    #[tabled(rename = "Last Seen")]
    last_seen: String,
}

impl From<&Host> for HostRow {
    fn from(host: &Host) -> Self {
        let reported = host.reported_state.as_ref();

        Self {
            // Cloud host ids run to 60-odd characters, which is wider than
            // every other column put together; cutting them short keeps the
            // listing readable, on character boundaries so a multi-byte id
            // from the API cannot be split.
            id: truncate_for_display(&host.id, CLOUD_HOST_ID_DISPLAY_CHARS),
            name: reported
                .and_then(|state| state.name.clone())
                .unwrap_or_else(|| UNKNOWN.to_string()),
            model: reported
                .and_then(|state| state.model.clone())
                .unwrap_or_else(|| UNKNOWN.to_string()),
            firmware: reported
                .and_then(|state| state.firmware_version.clone())
                .unwrap_or_else(|| UNKNOWN.to_string()),
            ip_address: host
                .ip_address
                .clone()
                .unwrap_or_else(|| NO_DATA.to_string()),
            host_type: host.host_type.clone(),
            owner: host.owner.to_string(),
            last_seen: host
                .last_connection_state_change
                .map(|changed| changed.format(LAST_SEEN_FORMAT).to_string())
                .unwrap_or_else(|| NO_DATA.to_string()),
        }
    }
}

/// Render the cloud host listing.
///
/// `--output json` answers with the hosts exactly as the API reported them,
/// rather than the columns the table happens to show, so a script reading the
/// output keeps every field.
///
/// # Arguments
///
/// * `hosts` - The hosts to render.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered text, without a trailing newline.
///
/// # Errors
///
/// Returns an error if the hosts cannot be serialized.
fn render_hosts(hosts: &[Host], format: OutputFormat) -> Result<String> {
    match format {
        OutputFormat::Json => render_output(hosts, format),
        OutputFormat::Table => {
            let rows: Vec<HostRow> = hosts.iter().map(HostRow::from).collect();
            render_vec_table(&rows, format)
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum CloudCommand {
    /// List all cloud-managed hosts/consoles
    Hosts,

    /// Get details for a specific host/console
    Host {
        /// Host/Console ID
        id: String,
    },
}

pub async fn handle_cloud_command(
    command: CloudCommand,
    client: &SiteManagerClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        CloudCommand::Hosts => {
            let hosts = client.get_hosts().await?;
            hosts_report(&hosts, output_format)?.print();
        }

        CloudCommand::Host { id } => {
            let host = client.get_host(&id).await?;
            host_report(&host, output_format)?.print();
        }
    }

    Ok(())
}

/// The report of `ufa cloud hosts`.
///
/// # Arguments
///
/// * `hosts` - Every host the account holds.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered hosts, together with the notes a person reads beside them.
///
/// # Errors
///
/// Returns an error if the hosts cannot be serialized.
fn hosts_report(hosts: &[Host], format: OutputFormat) -> Result<Report> {
    if hosts.is_empty() {
        return Ok(Report::of_document(
            "No cloud-managed hosts found.".to_string(),
        ));
    }

    let mut document = render_hosts(hosts, format)?;
    if matches!(format, OutputFormat::Table) {
        document.push_str(&format!(
            "\n\nTotal hosts: {}\n\nTo get details for a specific host, use: ufa cloud host <id>",
            hosts.len()
        ));
    }

    Ok(Report::of_document(document))
}

/// The report of `ufa cloud host <id>`.
///
/// # Arguments
///
/// * `host` - The host the user named.
/// * `format` - The output format the user asked for.
///
/// # Returns
///
/// The rendered host, together with the notes a person reads beside it.
///
/// # Errors
///
/// Returns an error if the host cannot be serialized.
fn host_report(host: &Host, format: OutputFormat) -> Result<Report> {
    let mut document = render_output(host, format)?;
    if matches!(format, OutputFormat::Table) {
        document.push_str(&format!(
            "\n\nCloud Console URL:\nhttps://unifi.ui.com/consoles/{}/network/default/dashboard",
            host.id
        ));
    }

    Ok(Report::of_document(document))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::site_manager::models::ReportedState;
    use chrono::TimeZone;

    /// Box-drawing corner produced by the crate's table renderer.
    const TABLE_CORNER: char = '┌';

    /// Every column the listing is expected to show.
    const HEADERS: [&str; 8] = [
        "ID",
        "Name",
        "Model",
        "Firmware",
        "IP Address",
        "Type",
        "Owner",
        "Last Seen",
    ];

    /// A host carrying a value in every column.
    fn test_host(name: &str) -> Host {
        Host {
            id: "900A6F00301A0000000004D1".to_string(),
            hardware_id: "0f2e1d3c-4b5a-6978-8796-a5b4c3d2e1f0".to_string(),
            host_type: "console".to_string(),
            ip_address: Some("192.168.1.1".to_string()),
            is_blocked: false,
            last_connection_state_change: Some(
                chrono::Utc
                    .with_ymd_and_hms(2026, 4, 20, 13, 45, 6)
                    .unwrap(),
            ),
            latest_backup_time: None,
            owner: true,
            registration_time: None,
            reported_state: Some(ReportedState {
                controllers: None,
                firmware_version: Some("4.0.6".to_string()),
                hostname: Some("unifi".to_string()),
                model: Some("UDM-Pro".to_string()),
                name: Some(name.to_string()),
            }),
            user_data: None,
        }
    }

    /// Every other listing in this CLI is drawn by the shared table renderer,
    /// so the cloud host listing reading as hand-spaced text is both an
    /// inconsistency and the reason its columns can come apart.
    #[test]
    fn the_host_listing_is_drawn_by_the_shared_table_renderer() {
        let rendered = render_hosts(&[test_host("udm-pro")], OutputFormat::Table)
            .expect("rendering the host listing must succeed");

        assert!(
            rendered.contains(TABLE_CORNER),
            "the listing must be drawn as a table, got:\n{rendered}"
        );
        for header in HEADERS {
            assert!(
                rendered.contains(header),
                "missing the {header} column, got:\n{rendered}"
            );
        }
        for value in [
            "900A6F00301A0000000004D1",
            "udm-pro",
            "UDM-Pro",
            "4.0.6",
            "192.168.1.1",
            "console",
            "true",
            "2026-04-20 13:45:06",
        ] {
            assert!(
                rendered.contains(value),
                "missing the value {value}, got:\n{rendered}"
            );
        }
    }

    /// A console the user named at length must not push every column after it
    /// out of line: a fixed-width format that pads but never truncates leaves
    /// the listing unreadable for exactly the people who labelled their gear.
    #[test]
    fn a_long_host_name_cannot_break_the_column_alignment() {
        let hosts = vec![test_host("edge"), test_host(&"n".repeat(120))];

        let rendered = render_hosts(&hosts, OutputFormat::Table)
            .expect("rendering the host listing must succeed");

        let widths: Vec<usize> = rendered.lines().map(|line| line.chars().count()).collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "every line must be the same width, got widths {widths:?} from:\n{rendered}"
        );
    }

    /// An id longer than the column budget is cut short on a character
    /// boundary rather than widening the table or splitting a character.
    #[test]
    fn an_over_long_host_id_is_cut_short_for_display() {
        let mut host = test_host("edge");
        host.id = "日".repeat(CLOUD_HOST_ID_DISPLAY_CHARS * 2);

        let rendered = render_hosts(&[host], OutputFormat::Table)
            .expect("rendering the host listing must succeed");

        assert!(
            rendered.contains(&format!("{}...", "日".repeat(CLOUD_HOST_ID_DISPLAY_CHARS))),
            "the id must be cut short at its display budget, got:\n{rendered}"
        );
    }

    /// `--output json` is how the listing gets piped into something else, so
    /// it must keep answering with the hosts as the API reported them rather
    /// than with the handful of fields the table happens to show.
    #[test]
    fn the_json_listing_stays_the_raw_hosts() {
        let rendered = render_hosts(&[test_host("udm-pro")], OutputFormat::Json)
            .expect("rendering the host listing must succeed");

        let parsed: serde_json::Value = serde_json::from_str(&rendered).unwrap_or_else(|error| {
            panic!("--output json must produce JSON ({error}):\n{rendered}")
        });

        assert_eq!(
            parsed[0]["hardwareId"], "0f2e1d3c-4b5a-6978-8796-a5b4c3d2e1f0",
            "--output json must keep fields the table drops, got:\n{rendered}"
        );
        assert_eq!(
            parsed[0]["reportedState"]["hostname"], "unifi",
            "--output json must keep the nested reported state, got:\n{rendered}"
        );
        assert_eq!(
            parsed[0]["isBlocked"], false,
            "--output json must keep every field, got:\n{rendered}"
        );
    }

    /// What a report says on standard error, one line for each note.
    fn stderr_of(report: &Report) -> String {
        report.notes().join("\n")
    }

    /// The finding this answers: `ufa cloud hosts --output json` on an
    /// account with no hosts printed a sentence in place of `[]`, so
    /// `jq -r '.[] | .id'` failed where it had nothing to read.
    #[test]
    fn an_account_with_no_hosts_answers_json_with_an_empty_array() {
        let report =
            hosts_report(&[], OutputFormat::Json).expect("rendering the host listing must succeed");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("--output json must answer with a document, got none"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(document).ok(),
            Some(serde_json::json!([])),
            "an account with no hosts must answer with an empty array, got:\n{document}"
        );
        assert!(
            stderr_of(&report).contains("No cloud-managed hosts"),
            "the note must say the account holds no hosts, got:\n{}",
            stderr_of(&report)
        );
    }

    /// A table of headings and no rows tells a person nothing. The sentence
    /// says it instead, on standard error, so a redirected table stays empty
    /// rather than holding a sentence.
    #[test]
    fn an_account_with_no_hosts_draws_no_table_and_says_so() {
        let report = hosts_report(&[], OutputFormat::Table)
            .expect("rendering the host listing must succeed");

        assert_eq!(
            report.document(),
            None,
            "an account with no hosts has no table to draw"
        );
        assert!(
            stderr_of(&report).contains("No cloud-managed hosts"),
            "the note must say the account holds no hosts, got:\n{}",
            stderr_of(&report)
        );
    }

    /// The count and the hint under the table are for a person. They go to
    /// standard error, so a table redirected to a file holds only the table.
    #[test]
    fn the_host_count_and_the_hint_stay_out_of_the_table() {
        let report = hosts_report(&[test_host("udm-pro")], OutputFormat::Table)
            .expect("rendering the host listing must succeed");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("a listing of one host must draw a table, got none"));
        assert!(
            !document.contains("Total hosts") && !document.contains("ufa cloud host <id>"),
            "the count and the hint belong beside the table, not in it, got:\n{document}"
        );

        let stderr = stderr_of(&report);
        assert!(
            stderr.contains("Total hosts: 1"),
            "the note must count the hosts, got:\n{stderr}"
        );
        assert!(
            stderr.contains("ufa cloud host <id>"),
            "the hint must name the command for one host, got:\n{stderr}"
        );
    }

    /// `--output json` is read by a program, and a count is a sentence the
    /// program does not need: the array holds it already.
    #[test]
    fn the_json_host_listing_carries_no_note() {
        let report = hosts_report(&[test_host("udm-pro")], OutputFormat::Json)
            .expect("rendering the host listing must succeed");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("--output json must answer with a document, got none"));
        assert!(
            serde_json::from_str::<serde_json::Value>(document).is_ok(),
            "--output json must produce JSON, got:\n{document}"
        );
        assert!(
            report.notes().is_empty(),
            "a JSON listing of hosts needs no note, got:\n{}",
            stderr_of(&report)
        );
    }

    /// The dashboard URL of a host is a hint for a person, so it goes to
    /// standard error and the table holds only the host.
    #[test]
    fn the_console_url_of_a_host_stays_out_of_its_table() {
        let host = test_host("udm-pro");

        let report =
            host_report(&host, OutputFormat::Table).expect("rendering one host must succeed");

        let document = report
            .document()
            .unwrap_or_else(|| panic!("one host must draw a table, got none"));
        assert!(
            !document.contains("unifi.ui.com/consoles"),
            "the dashboard URL belongs beside the table, not in it, got:\n{document}"
        );
        assert!(
            stderr_of(&report).contains(&format!(
                "https://unifi.ui.com/consoles/{}/network/default/dashboard",
                host.id
            )),
            "the note must give the dashboard URL of the host, got:\n{}",
            stderr_of(&report)
        );
    }
}
