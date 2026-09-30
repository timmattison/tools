use anyhow::Result;
use clap::Subcommand;
use tabled::Tabled;
use uuid::Uuid;

use crate::{
    client::UnifiClient,
    models::{Client, ClientAction, Page},
    output::{print_output, render_page_listing, OutputFormat, Report},
    site_helper::get_site_id_or_prompt,
};

/// Shown in place of a value the client kind does not have at all, such as
/// the MAC address of a VPN client.
const NOT_APPLICABLE: &str = "N/A";

#[derive(Subcommand, Debug)]
pub enum ClientsCommand {
    /// List connected clients on a site
    List {
        /// Maximum number of clients to return
        #[clap(long, default_value = "25")]
        limit: u32,

        /// Offset for pagination
        #[clap(long, default_value = "0")]
        offset: u64,

        /// Filter expression
        #[clap(long)]
        filter: Option<String>,
    },

    /// Get client details
    Get {
        /// Client ID
        client_id: Uuid,
    },

    /// Authorize guest access for a client
    AuthorizeGuest {
        /// Client ID
        client_id: Uuid,

        /// Time limit in minutes
        #[clap(long)]
        time_limit_minutes: Option<u64>,

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

    /// Unauthorize guest access for a client
    UnauthorizeGuest {
        /// Client ID
        client_id: Uuid,
    },
}

#[derive(Tabled, serde::Serialize)]
struct ClientRow {
    #[tabled(rename = "ID")]
    id: String,
    #[tabled(rename = "Name")]
    name: String,
    #[tabled(rename = "Type")]
    client_type: String,
    #[tabled(rename = "IP Address")]
    ip_address: String,
    #[tabled(rename = "MAC Address")]
    mac_address: String,
    #[tabled(rename = "Connected At")]
    connected_at: String,
}

/// Render an address a client may not have at all.
///
/// # Arguments
///
/// * `address` - The address the controller reported, if it reported one.
///
/// # Returns
///
/// The address as the controller spelled it, or an empty cell.
fn address_or_blank<A: std::fmt::Display>(address: Option<&A>) -> String {
    address.map(ToString::to_string).unwrap_or_default()
}

impl From<&Client> for ClientRow {
    /// Describe one client as a row of the listing.
    ///
    /// # Arguments
    ///
    /// * `client` - The client the controller reported.
    fn from(client: &Client) -> Self {
        match client {
            Client::Wired(c) => ClientRow {
                id: c.id.to_string(),
                name: c.name.clone(),
                client_type: "WIRED".to_string(),
                ip_address: address_or_blank(c.ip_address.as_ref()),
                mac_address: c.mac_address.to_string(),
                connected_at: c.connected_at.clone().unwrap_or_default(),
            },
            Client::Wireless(c) => ClientRow {
                id: c.id.to_string(),
                name: c.name.clone(),
                client_type: "WIRELESS".to_string(),
                ip_address: address_or_blank(c.ip_address.as_ref()),
                mac_address: c.mac_address.to_string(),
                connected_at: c.connected_at.clone().unwrap_or_default(),
            },
            Client::Vpn(c) => ClientRow {
                id: c.id.to_string(),
                name: c.name.clone(),
                client_type: "VPN".to_string(),
                ip_address: address_or_blank(c.ip_address.as_ref()),
                mac_address: NOT_APPLICABLE.to_string(),
                connected_at: c.connected_at.clone().unwrap_or_default(),
            },
            Client::Teleport(c) => ClientRow {
                id: c.id.to_string(),
                name: c.name.clone(),
                client_type: "TELEPORT".to_string(),
                ip_address: address_or_blank(c.ip_address.as_ref()),
                mac_address: NOT_APPLICABLE.to_string(),
                connected_at: c.connected_at.clone().unwrap_or_default(),
            },
            // A client kind this build does not know still belongs in the
            // listing, described with whatever the controller did say about it.
            Client::Unknown(c) => ClientRow {
                id: c.id.map(|id| id.to_string()).unwrap_or_default(),
                name: c.name.clone().unwrap_or_default(),
                client_type: c.client_type.clone(),
                ip_address: address_or_blank(c.ip_address.as_ref()),
                mac_address: c
                    .mac_address
                    .as_ref()
                    .map_or_else(|| NOT_APPLICABLE.to_string(), ToString::to_string),
                connected_at: c.connected_at.clone().unwrap_or_default(),
            },
        }
    }
}

pub async fn handle_clients_command(
    command: ClientsCommand,
    site_id: Option<Uuid>,
    client: &UnifiClient,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        ClientsCommand::List {
            limit,
            offset,
            filter,
        } => list_clients(client, site_id, limit, offset, filter, output_format)
            .await
            .map(Report::print),
        ClientsCommand::Get { client_id } => {
            get_client(client, site_id, client_id, output_format).await
        }
        ClientsCommand::AuthorizeGuest {
            client_id,
            time_limit_minutes,
            data_usage_limit_mbytes,
            rx_rate_limit_kbps,
            tx_rate_limit_kbps,
        } => authorize_guest(
            client,
            site_id,
            client_id,
            time_limit_minutes,
            data_usage_limit_mbytes,
            rx_rate_limit_kbps,
            tx_rate_limit_kbps,
        )
        .await
        .map(Report::print),
        ClientsCommand::UnauthorizeGuest { client_id } => {
            unauthorize_guest(client, site_id, client_id)
                .await
                .map(Report::print)
        }
    }
}

/// List one page of the clients connected to a site.
///
/// # Arguments
///
/// * `client` - The controller client to list the clients with.
/// * `site_id` - The site the user named, if any.
/// * `limit` - How many clients to ask for.
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
async fn list_clients(
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
    let path = format!("sites/{}/clients", site_id);
    let page: Page<Client> = client.get_with_params(&path, &params).await?;

    render_page_listing::<Client, ClientRow>(&page, output_format)
}

async fn get_client(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
    output_format: OutputFormat,
) -> Result<()> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}", site_id, client_id);
    let client_details: Client = client.get(&path).await?;

    print_output(&client_details, output_format)?;
    Ok(())
}

/// Ask the controller to authorize guest access for a client.
///
/// # Arguments
///
/// * `client` - The controller client to send the request with.
/// * `site_id` - The site the user named, if any.
/// * `client_id` - The client to authorize.
/// * `time_limit_minutes` - How long the access lasts, if it is limited.
/// * `data_usage_limit_mbytes` - How much data the client can use, if it is
///   limited.
/// * `rx_rate_limit_kbps` - The download rate limit, if any.
/// * `tx_rate_limit_kbps` - The upload rate limit, if any.
///
/// # Returns
///
/// The report that says the controller accepted the request.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, or if the request fails.
async fn authorize_guest(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
    time_limit_minutes: Option<u64>,
    data_usage_limit_mbytes: Option<u64>,
    rx_rate_limit_kbps: Option<u64>,
    tx_rate_limit_kbps: Option<u64>,
) -> Result<Report> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}/actions", site_id, client_id);
    let action = ClientAction::AuthorizeGuestAccess {
        time_limit_minutes,
        data_usage_limit_mbytes,
        rx_rate_limit_kbps,
        tx_rate_limit_kbps,
    };

    let _: serde_json::Value = client.post(&path, &action).await?;
    Ok(Report::of_note("Guest access authorized successfully"))
}

/// Ask the controller to end the guest access of a client.
///
/// # Arguments
///
/// * `client` - The controller client to send the request with.
/// * `site_id` - The site the user named, if any.
/// * `client_id` - The client whose guest access ends.
///
/// # Returns
///
/// The report that says the controller accepted the request.
///
/// # Errors
///
/// Returns an error if the site cannot be resolved, or if the request fails.
async fn unauthorize_guest(
    client: &UnifiClient,
    site_id: Option<Uuid>,
    client_id: Uuid,
) -> Result<Report> {
    let site_id = get_site_id_or_prompt(client, site_id).await?;
    let path = format!("sites/{}/clients/{}/actions", site_id, client_id);
    let action = ClientAction::UnauthorizeGuestAccess;

    let _: serde_json::Value = client.post(&path, &action).await?;
    Ok(Report::of_note("Guest access unauthorized successfully"))
}

#[cfg(test)]
mod row_tests {
    use super::*;
    use crate::models::{
        ClientAccess, TeleportClient, UnknownClient, VpnClient, WiredClient, WirelessClient,
    };

    /// What a cell says when the client kind has no such address at all.
    ///
    /// Spelled out here rather than read from the constant it stands for. A
    /// test that quotes the value under test proves only that the value
    /// equals itself, so this is the expectation and the code has to meet it.
    const NO_ADDRESS: &str = "N/A";

    /// The network address a sample client reports.
    const AN_IP: &str = "192.168.1.50";

    /// The hardware address a sample client reports.
    const A_MAC: &str = "aa:bb:cc:dd:ee:ff";

    /// When a sample client connected.
    const A_TIMESTAMP: &str = "2026-09-07T03:27:25Z";

    /// The name a sample client reports.
    const A_NAME: &str = "a client";

    /// The `type` of a client kind no build of this tool knows, standing in
    /// for whatever Ubiquiti ships in the next firmware.
    const A_FUTURE_KIND: &str = "FUTURE_FIRMWARE_KIND";

    /// The id of each sample. One per kind, so a row that took its id from
    /// the wrong client is visible in the failure.
    const WIRED_ID: Uuid = Uuid::from_u128(0x11);
    /// The id of the wireless sample. See [`WIRED_ID`].
    const WIRELESS_ID: Uuid = Uuid::from_u128(0x22);
    /// The id of the VPN sample. See [`WIRED_ID`].
    const VPN_ID: Uuid = Uuid::from_u128(0x33);
    /// The id of the Teleport sample. See [`WIRED_ID`].
    const TELEPORT_ID: Uuid = Uuid::from_u128(0x44);
    /// The id of the unrecognized sample. See [`WIRED_ID`].
    const UNKNOWN_ID: Uuid = Uuid::from_u128(0x55);
    /// The device a wired or wireless sample hangs off.
    const UPLINK_ID: Uuid = Uuid::from_u128(0x66);

    /// Name the variant `client` is.
    ///
    /// The match carries no catch-all arm on purpose. A sixth `Client`
    /// variant fails to compile here, which sends whoever adds it to this
    /// file for a sample and a row test of its own. Without that, a new kind
    /// of client would reach the listing with nothing said about how it
    /// renders.
    ///
    /// # Arguments
    ///
    /// * `client` - The client to name.
    ///
    /// # Returns
    ///
    /// The name of the variant, as the enum declares it.
    fn variant_name(client: &Client) -> &'static str {
        match client {
            Client::Wired(_) => "Wired",
            Client::Wireless(_) => "Wireless",
            Client::Vpn(_) => "Vpn",
            Client::Teleport(_) => "Teleport",
            Client::Unknown(_) => "Unknown",
        }
    }

    /// Every name [`variant_name`] can answer with, in the order `Client`
    /// declares its variants.
    const EVERY_VARIANT: [&str; 5] = ["Wired", "Wireless", "Vpn", "Teleport", "Unknown"];

    /// A wired client that reported everything it can report.
    fn wired() -> Client {
        Client::Wired(WiredClient {
            id: WIRED_ID,
            name: A_NAME.to_string(),
            connected_at: Some(A_TIMESTAMP.to_string()),
            ip_address: Some(AN_IP.into()),
            mac_address: A_MAC.into(),
            uplink_device_id: UPLINK_ID,
            access: ClientAccess::Default,
        })
    }

    /// A wireless client that reported everything it can report.
    fn wireless() -> Client {
        Client::Wireless(WirelessClient {
            id: WIRELESS_ID,
            name: A_NAME.to_string(),
            connected_at: Some(A_TIMESTAMP.to_string()),
            ip_address: Some(AN_IP.into()),
            mac_address: A_MAC.into(),
            uplink_device_id: UPLINK_ID,
            access: ClientAccess::Default,
        })
    }

    /// A VPN client. The kind carries no hardware address at all.
    fn vpn() -> Client {
        Client::Vpn(VpnClient {
            id: VPN_ID,
            name: A_NAME.to_string(),
            connected_at: Some(A_TIMESTAMP.to_string()),
            ip_address: Some(AN_IP.into()),
            access: ClientAccess::Default,
        })
    }

    /// A Teleport client. The kind carries no hardware address at all.
    fn teleport() -> Client {
        Client::Teleport(TeleportClient {
            id: TELEPORT_ID,
            name: A_NAME.to_string(),
            connected_at: Some(A_TIMESTAMP.to_string()),
            ip_address: Some(AN_IP.into()),
            access: ClientAccess::Default,
        })
    }

    /// A client of a kind this build does not know, which nonetheless
    /// reported every field the known kinds share.
    fn unknown() -> Client {
        Client::Unknown(UnknownClient {
            client_type: A_FUTURE_KIND.to_string(),
            id: Some(UNKNOWN_ID),
            name: Some(A_NAME.to_string()),
            connected_at: Some(A_TIMESTAMP.to_string()),
            ip_address: Some(AN_IP.into()),
            mac_address: Some(A_MAC.into()),
            other_fields: serde_json::Map::new(),
        })
    }

    /// One sample of every kind, in the order [`EVERY_VARIANT`] names them.
    fn one_of_every_kind() -> Vec<Client> {
        vec![wired(), wireless(), vpn(), teleport(), unknown()]
    }

    /// Every variant of `Client` has a sample here and a row test below.
    ///
    /// Three separate things break when a sixth variant arrives: the match in
    /// [`variant_name`] stops compiling, this list is one sample short, and
    /// [`EVERY_VARIANT`] is one name short. A client kind therefore cannot
    /// reach the listing untested by accident.
    #[test]
    fn every_client_variant_has_a_sample_of_its_own() {
        let named: Vec<&str> = one_of_every_kind().iter().map(variant_name).collect();

        assert_eq!(
            named, EVERY_VARIANT,
            "every Client variant needs a sample here, in the order the enum declares them"
        );
    }

    /// A wired client is the ordinary case: it has both addresses, and both
    /// belong in the row.
    #[test]
    fn a_wired_client_row_carries_its_type_and_both_addresses() {
        let row = ClientRow::from(&wired());

        assert_eq!(row.id, WIRED_ID.to_string());
        assert_eq!(row.name, A_NAME);
        assert_eq!(row.client_type, "WIRED");
        assert_eq!(row.ip_address, AN_IP);
        assert_eq!(row.mac_address, A_MAC);
        assert_eq!(row.connected_at, A_TIMESTAMP);
    }

    /// A wireless client carries the same fields as a wired one and must not
    /// be shown as one.
    #[test]
    fn a_wireless_client_row_carries_its_type_and_both_addresses() {
        let row = ClientRow::from(&wireless());

        assert_eq!(row.id, WIRELESS_ID.to_string());
        assert_eq!(row.name, A_NAME);
        assert_eq!(row.client_type, "WIRELESS");
        assert_eq!(row.ip_address, AN_IP);
        assert_eq!(row.mac_address, A_MAC);
        assert_eq!(row.connected_at, A_TIMESTAMP);
    }

    /// A VPN client reaches the controller over a tunnel, so it has no
    /// hardware address for the column to hold. An empty cell would read as
    /// "the controller did not say", which is a different thing.
    #[test]
    fn a_vpn_client_row_says_it_has_no_hardware_address() {
        let row = ClientRow::from(&vpn());

        assert_eq!(row.id, VPN_ID.to_string());
        assert_eq!(row.client_type, "VPN");
        assert_eq!(row.ip_address, AN_IP);
        assert_eq!(row.mac_address, NO_ADDRESS);
        assert_eq!(row.connected_at, A_TIMESTAMP);
    }

    /// A Teleport client has no hardware address either, and it is its own
    /// kind rather than a VPN.
    #[test]
    fn a_teleport_client_row_says_it_has_no_hardware_address() {
        let row = ClientRow::from(&teleport());

        assert_eq!(row.id, TELEPORT_ID.to_string());
        assert_eq!(row.client_type, "TELEPORT");
        assert_eq!(row.ip_address, AN_IP);
        assert_eq!(row.mac_address, NO_ADDRESS);
        assert_eq!(row.connected_at, A_TIMESTAMP);
    }

    /// A client kind this build does not know still belongs in the listing,
    /// described by the `type` the controller sent rather than by a guess.
    #[test]
    fn an_unknown_client_row_carries_the_type_the_controller_named() {
        let row = ClientRow::from(&unknown());

        assert_eq!(row.id, UNKNOWN_ID.to_string());
        assert_eq!(row.name, A_NAME);
        assert_eq!(row.client_type, A_FUTURE_KIND);
        assert_eq!(row.ip_address, AN_IP);
        assert_eq!(row.mac_address, A_MAC);
        assert_eq!(row.connected_at, A_TIMESTAMP);
    }

    /// A kind that has a network address, on a run where the controller
    /// reported none, gets an empty cell. The column stays empty rather than
    /// claiming the client has no address of that kind.
    #[test]
    fn a_client_that_reported_no_network_address_gets_an_empty_cell() {
        let row = ClientRow::from(&Client::Wired(WiredClient {
            id: WIRED_ID,
            name: A_NAME.to_string(),
            connected_at: None,
            ip_address: None,
            mac_address: A_MAC.into(),
            uplink_device_id: UPLINK_ID,
            access: ClientAccess::Default,
        }));

        assert_eq!(row.ip_address, "");
        assert_eq!(row.connected_at, "");
        assert_eq!(
            row.mac_address, A_MAC,
            "the address the controller did send must survive"
        );
    }

    /// An unrecognized kind shares no field with the known ones except its
    /// `type`, so every other cell has to fall back on its own.
    #[test]
    fn an_unknown_client_that_reported_nothing_but_a_type_gets_empty_cells() {
        let row = ClientRow::from(&Client::Unknown(UnknownClient {
            client_type: A_FUTURE_KIND.to_string(),
            id: None,
            name: None,
            connected_at: None,
            ip_address: None,
            mac_address: None,
            other_fields: serde_json::Map::new(),
        }));

        assert_eq!(row.id, "");
        assert_eq!(row.name, "");
        assert_eq!(row.client_type, A_FUTURE_KIND);
        assert_eq!(row.ip_address, "");
        assert_eq!(row.connected_at, "");
        assert_eq!(
            row.mac_address, NO_ADDRESS,
            "an unrecognized kind is not known to have a hardware address"
        );
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::test_server::{json_response, TestServer};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// How many clients the pretend controller says it holds.
    const TOTAL: &str = "100";

    /// One page of the client collection: a single item, out of a hundred.
    const ONE_CLIENT_OF_A_HUNDRED: &str = r#"{
        "offset": 0, "limit": 1, "count": 1, "totalCount": 100,
        "data": [
            {
                "type": "WIRED",
                "id": "00000000-0000-0000-0000-000000000002",
                "name": "nas",
                "ipAddress": "192.168.1.10",
                "macAddress": "00:11:22:33:44:10",
                "uplinkDeviceId": "00000000-0000-0000-0000-000000000003",
                "access": { "type": "DEFAULT" }
            }
        ]
    }"#;

    /// A page that holds a single client of a hundred looks exactly like the whole
    /// listing of a controller that has one. The note that tells the two apart
    /// is decided in `output`, and this proves the client listing asks for it: a
    /// command that renders its page on its own answers with no note at all.
    #[tokio::test]
    async fn a_short_page_of_clients_reports_what_it_left_out() {
        let controller = TestServer::replying(&json_response(ONE_CLIENT_OF_A_HUNDRED)).await;
        let client = UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let listing = list_clients(
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

#[cfg(test)]
mod action_tests {
    use super::*;
    use crate::test_server::{empty_json, TestServer, INTEGRATION_API};

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// A client for the controller a test runs.
    fn client_for(controller: &TestServer) -> UnifiClient {
        UnifiClient::new(controller.origin(), API_KEY, false)
            .expect("a loopback URL must build a client")
    }

    /// Guest authorization has no document to give. The sentence that says
    /// the controller accepted it is a note, so it goes to standard error.
    #[tokio::test]
    async fn authorizing_a_guest_reports_on_standard_error_only() {
        let controller = TestServer::replying(&empty_json()).await;

        let report = authorize_guest(
            &client_for(&controller),
            Some(Uuid::new_v4()),
            Uuid::new_v4(),
            Some(60),
            None,
            None,
            None,
        )
        .await
        .expect("the controller accepted the authorization");

        assert_eq!(
            report.document(),
            None,
            "an authorization must put nothing on standard output"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("authorized successfully")),
            "the note must say the guest is authorized, got {:?}",
            report.notes()
        );
    }

    /// The same rule for the end of guest access.
    #[tokio::test]
    async fn unauthorizing_a_guest_reports_on_standard_error_only() {
        let controller = TestServer::replying(&empty_json()).await;

        let report = unauthorize_guest(
            &client_for(&controller),
            Some(Uuid::new_v4()),
            Uuid::new_v4(),
        )
        .await
        .expect("the controller accepted the end of guest access");

        assert_eq!(
            report.document(),
            None,
            "the end of guest access must put nothing on standard output"
        );
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("unauthorized successfully")),
            "the note must say the guest access ended, got {:?}",
            report.notes()
        );
    }

    /// Send `command` for a site and a client of its own, and return both.
    ///
    /// # Arguments
    ///
    /// * `controller` - The controller the command goes to.
    /// * `command` - Builds the command from the id of the client.
    ///
    /// # Returns
    ///
    /// The site id and the client id the command named.
    async fn run_for_a_client(
        controller: &TestServer,
        command: impl FnOnce(Uuid) -> ClientsCommand,
    ) -> (Uuid, Uuid) {
        let site_id = Uuid::new_v4();
        let client_id = Uuid::new_v4();

        handle_clients_command(
            command(client_id),
            Some(site_id),
            &client_for(controller),
            OutputFormat::Table,
        )
        .await
        .expect("the controller accepted the action");

        (site_id, client_id)
    }

    /// `clients authorize-guest` with every limit set posts the
    /// `AUTHORIZE_GUEST_ACCESS` action with each limit under its own key.
    ///
    /// The four limits are all counts, so a limit that goes under the key of
    /// another limit still compiles. Each limit here has a value of its own,
    /// so a limit under the wrong key fails the test. The names of the keys
    /// come from the UniFi integration API.
    #[tokio::test]
    async fn authorizing_a_guest_with_every_limit_posts_each_limit_under_its_own_key() {
        let controller = TestServer::replying(&empty_json()).await;

        let (site_id, client_id) =
            run_for_a_client(&controller, |client_id| ClientsCommand::AuthorizeGuest {
                client_id,
                time_limit_minutes: Some(60),
                data_usage_limit_mbytes: Some(1024),
                rx_rate_limit_kbps: Some(2000),
                tx_rate_limit_kbps: Some(500),
            })
            .await;

        controller.assert_one_json_request(
            &format!("POST {INTEGRATION_API}/sites/{site_id}/clients/{client_id}/actions HTTP/1.1"),
            &serde_json::json!({
                "action": "AUTHORIZE_GUEST_ACCESS",
                "timeLimitMinutes": 60,
                "dataUsageLimitMBytes": 1024,
                "rxRateLimitKbps": 2000,
                "txRateLimitKbps": 500
            }),
        );
    }

    /// `clients authorize-guest` with no limit posts the action alone. A
    /// limit the user did not give is left out of the body, not sent as
    /// `null`.
    #[tokio::test]
    async fn authorizing_a_guest_with_no_limit_posts_the_action_alone() {
        let controller = TestServer::replying(&empty_json()).await;

        let (site_id, client_id) =
            run_for_a_client(&controller, |client_id| ClientsCommand::AuthorizeGuest {
                client_id,
                time_limit_minutes: None,
                data_usage_limit_mbytes: None,
                rx_rate_limit_kbps: None,
                tx_rate_limit_kbps: None,
            })
            .await;

        controller.assert_one_json_request(
            &format!("POST {INTEGRATION_API}/sites/{site_id}/clients/{client_id}/actions HTTP/1.1"),
            &serde_json::json!({ "action": "AUTHORIZE_GUEST_ACCESS" }),
        );
    }

    /// `clients unauthorize-guest` posts the `UNAUTHORIZE_GUEST_ACCESS`
    /// action to the actions path of the client the user named.
    #[tokio::test]
    async fn unauthorizing_a_guest_posts_the_unauthorize_action_to_the_client() {
        let controller = TestServer::replying(&empty_json()).await;

        let (site_id, client_id) = run_for_a_client(&controller, |client_id| {
            ClientsCommand::UnauthorizeGuest { client_id }
        })
        .await;

        controller.assert_one_json_request(
            &format!("POST {INTEGRATION_API}/sites/{site_id}/clients/{client_id}/actions HTTP/1.1"),
            &serde_json::json!({ "action": "UNAUTHORIZE_GUEST_ACCESS" }),
        );
    }
}
