use serde::{Deserialize, Serialize};
use uuid::Uuid;

// Common pagination types
#[derive(Debug, Deserialize, Serialize)]
pub struct Page<T> {
    pub offset: u64,
    pub limit: u32,
    pub count: u32,
    #[serde(rename = "totalCount")]
    pub total_count: u64,
    pub data: Vec<T>,
}

// Site models
#[derive(Debug, Deserialize, Serialize)]
pub struct Site {
    pub id: Uuid,
    #[serde(rename = "internalReference")]
    pub internal_reference: String,
    pub name: String,
}

// Device models
#[derive(Debug, Deserialize, Serialize)]
pub struct Device {
    pub id: Uuid,
    pub name: String,
    pub model: String,
    #[serde(rename = "macAddress")]
    pub mac_address: String,
    #[serde(rename = "ipAddress")]
    pub ip_address: String,
    pub state: DeviceState,
    pub features: Vec<DeviceFeature>,
    pub interfaces: Vec<DeviceInterface>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceDetails {
    pub id: Uuid,
    pub name: String,
    pub model: String,
    pub supported: bool,
    #[serde(rename = "macAddress")]
    pub mac_address: String,
    #[serde(rename = "ipAddress")]
    pub ip_address: String,
    pub state: DeviceState,
    #[serde(rename = "firmwareVersion")]
    pub firmware_version: String,
    #[serde(rename = "firmwareUpdatable")]
    pub firmware_updatable: bool,
    #[serde(rename = "adoptedAt")]
    pub adopted_at: Option<String>,
    #[serde(rename = "provisionedAt")]
    pub provisioned_at: Option<String>,
    #[serde(rename = "configurationId")]
    pub configuration_id: String,
    pub uplink: Option<DeviceUplink>,
    pub features: serde_json::Value,
    pub interfaces: DeviceInterfaces,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeviceState {
    Online,
    Offline,
    PendingAdoption,
    Updating,
    GettingReady,
    Adopting,
    Deleting,
    ConnectionInterrupted,
    Isolated,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceFeature {
    Switching,
    AccessPoint,
    Gateway,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceInterface {
    Ports,
    Radios,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceUplink {
    #[serde(rename = "deviceId")]
    pub device_id: Uuid,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceInterfaces {
    pub ports: Option<Vec<Port>>,
    pub radios: Option<Vec<WirelessRadio>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Port {
    pub idx: u32,
    pub state: PortState,
    pub connector: PortConnector,
    #[serde(rename = "maxSpeedMbps")]
    pub max_speed_mbps: u32,
    #[serde(rename = "speedMbps")]
    pub speed_mbps: Option<u32>,
    pub poe: Option<PortPoE>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PortState {
    Up,
    Down,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PortConnector {
    Rj45,
    Sfp,
    Sfpplus,
    Sfp28,
    Qsfp28,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PortPoE {
    pub standard: PoEStandard,
    #[serde(rename = "type")]
    pub poe_type: u8,
    pub enabled: bool,
    pub state: PoEState,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum PoEStandard {
    #[serde(rename = "802.3af")]
    Af,
    #[serde(rename = "802.3at")]
    At,
    #[serde(rename = "802.3bt")]
    Bt,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PoEState {
    Up,
    Down,
    Limited,
    Unknown,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WirelessRadio {
    #[serde(rename = "wlanStandard")]
    pub wlan_standard: WlanStandard,
    #[serde(rename = "frequencyGHz")]
    pub frequency_ghz: String,
    #[serde(rename = "channelWidthMHz")]
    pub channel_width_mhz: u32,
    pub channel: Option<u32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum WlanStandard {
    #[serde(rename = "802.11a")]
    A,
    #[serde(rename = "802.11b")]
    B,
    #[serde(rename = "802.11g")]
    G,
    #[serde(rename = "802.11n")]
    N,
    #[serde(rename = "802.11ac")]
    Ac,
    #[serde(rename = "802.11ax")]
    Ax,
    #[serde(rename = "802.11be")]
    Be,
}

// Device statistics
#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceStatistics {
    #[serde(rename = "uptimeSec")]
    pub uptime_sec: Option<u64>,
    #[serde(rename = "lastHeartbeatAt")]
    pub last_heartbeat_at: Option<String>,
    #[serde(rename = "nextHeartbeatAt")]
    pub next_heartbeat_at: Option<String>,
    #[serde(rename = "loadAverage1Min")]
    pub load_average_1min: Option<f64>,
    #[serde(rename = "loadAverage5Min")]
    pub load_average_5min: Option<f64>,
    #[serde(rename = "loadAverage15Min")]
    pub load_average_15min: Option<f64>,
    #[serde(rename = "cpuUtilizationPct")]
    pub cpu_utilization_pct: Option<f64>,
    #[serde(rename = "memoryUtilizationPct")]
    pub memory_utilization_pct: Option<f64>,
    pub uplink: Option<UplinkStatistics>,
    pub interfaces: DeviceInterfaceStatistics,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UplinkStatistics {
    #[serde(rename = "txRateBps")]
    pub tx_rate_bps: Option<u64>,
    #[serde(rename = "rxRateBps")]
    pub rx_rate_bps: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeviceInterfaceStatistics {
    pub radios: Option<Vec<RadioStatistics>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RadioStatistics {
    #[serde(rename = "frequencyGHz")]
    pub frequency_ghz: f64,
    #[serde(rename = "txRetriesPct")]
    pub tx_retries_pct: Option<f64>,
}

// Client models
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum Client {
    #[serde(rename = "WIRED")]
    Wired(WiredClient),
    #[serde(rename = "WIRELESS")]
    Wireless(WirelessClient),
    #[serde(rename = "VPN")]
    Vpn(VpnClient),
    #[serde(rename = "TELEPORT")]
    Teleport(TeleportClient),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WiredClient {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "connectedAt")]
    pub connected_at: Option<String>,
    #[serde(rename = "ipAddress")]
    pub ip_address: Option<String>,
    #[serde(rename = "macAddress")]
    pub mac_address: String,
    #[serde(rename = "uplinkDeviceId")]
    pub uplink_device_id: Uuid,
    pub access: ClientAccess,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WirelessClient {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "connectedAt")]
    pub connected_at: Option<String>,
    #[serde(rename = "ipAddress")]
    pub ip_address: Option<String>,
    #[serde(rename = "macAddress")]
    pub mac_address: String,
    #[serde(rename = "uplinkDeviceId")]
    pub uplink_device_id: Uuid,
    pub access: ClientAccess,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VpnClient {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "connectedAt")]
    pub connected_at: Option<String>,
    #[serde(rename = "ipAddress")]
    pub ip_address: Option<String>,
    pub access: ClientAccess,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TeleportClient {
    pub id: Uuid,
    pub name: String,
    #[serde(rename = "connectedAt")]
    pub connected_at: Option<String>,
    #[serde(rename = "ipAddress")]
    pub ip_address: Option<String>,
    pub access: ClientAccess,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum ClientAccess {
    #[serde(rename = "DEFAULT")]
    Default,
    #[serde(rename = "GUEST")]
    Guest { authorized: bool },
}

// Voucher models
#[derive(Debug, Deserialize, Serialize)]
pub struct Voucher {
    pub id: Uuid,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub name: String,
    pub code: String,
    #[serde(rename = "authorizedGuestLimit")]
    pub authorized_guest_limit: Option<u64>,
    #[serde(rename = "authorizedGuestCount")]
    pub authorized_guest_count: u64,
    #[serde(rename = "activatedAt")]
    pub activated_at: Option<String>,
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<String>,
    pub expired: bool,
    #[serde(rename = "timeLimitMinutes")]
    pub time_limit_minutes: u64,
    #[serde(rename = "dataUsageLimitMBytes")]
    pub data_usage_limit_mbytes: Option<u64>,
    #[serde(rename = "rxRateLimitKbps")]
    pub rx_rate_limit_kbps: Option<u64>,
    #[serde(rename = "txRateLimitKbps")]
    pub tx_rate_limit_kbps: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct VoucherCreateRequest {
    pub count: u32,
    pub name: String,
    #[serde(
        rename = "authorizedGuestLimit",
        skip_serializing_if = "Option::is_none"
    )]
    pub authorized_guest_limit: Option<u64>,
    #[serde(rename = "timeLimitMinutes")]
    pub time_limit_minutes: u64,
    #[serde(
        rename = "dataUsageLimitMBytes",
        skip_serializing_if = "Option::is_none"
    )]
    pub data_usage_limit_mbytes: Option<u64>,
    #[serde(rename = "rxRateLimitKbps", skip_serializing_if = "Option::is_none")]
    pub rx_rate_limit_kbps: Option<u64>,
    #[serde(rename = "txRateLimitKbps", skip_serializing_if = "Option::is_none")]
    pub tx_rate_limit_kbps: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct VoucherCreateResponse {
    pub vouchers: Vec<Voucher>,
}

#[derive(Debug, Deserialize)]
pub struct VoucherDeletionResults {
    #[serde(rename = "vouchersDeleted")]
    pub vouchers_deleted: u64,
}

// Application info
#[derive(Debug, Deserialize, Serialize)]
pub struct ApplicationInfo {
    #[serde(rename = "applicationVersion")]
    pub application_version: String,
}

// Action models
#[derive(Debug, Serialize)]
#[serde(tag = "action")]
pub enum DeviceAction {
    #[serde(rename = "RESTART")]
    Restart,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
pub enum PortAction {
    #[serde(rename = "POWER_CYCLE")]
    PowerCycle,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action")]
pub enum ClientAction {
    #[serde(rename = "AUTHORIZE_GUEST_ACCESS")]
    AuthorizeGuestAccess {
        #[serde(rename = "timeLimitMinutes", skip_serializing_if = "Option::is_none")]
        time_limit_minutes: Option<u64>,
        #[serde(
            rename = "dataUsageLimitMBytes",
            skip_serializing_if = "Option::is_none"
        )]
        data_usage_limit_mbytes: Option<u64>,
        #[serde(rename = "rxRateLimitKbps", skip_serializing_if = "Option::is_none")]
        rx_rate_limit_kbps: Option<u64>,
        #[serde(rename = "txRateLimitKbps", skip_serializing_if = "Option::is_none")]
        tx_rate_limit_kbps: Option<u64>,
    },
    #[serde(rename = "UNAUTHORIZE_GUEST_ACCESS")]
    UnauthorizeGuestAccess,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    /// A value no build of this tool knows about, standing in for whatever
    /// Ubiquiti ships in the next firmware.
    const FUTURE_VALUE: &str = "FUTURE_FIRMWARE_VALUE";

    /// Where the production half of this file ends and the tests begin.
    const TEST_MODULE_ATTRIBUTE: &str = "#[cfg(test)]";

    /// Deserialize `raw` as a bare JSON string into `T` and serialize it back.
    ///
    /// # Arguments
    ///
    /// * `raw` - The value the controller sent, without JSON quoting.
    ///
    /// # Returns
    ///
    /// Whatever `T` serializes back to, without JSON quoting.
    ///
    /// # Panics
    ///
    /// Panics if the value cannot be read into `T` or written back out.
    fn round_trip<T: DeserializeOwned + Serialize>(raw: &str) -> String {
        let json = serde_json::to_string(raw).expect("quoting a string must succeed");
        let parsed: T = serde_json::from_str(&json).unwrap_or_else(|error| {
            panic!(
                "{} must accept the unrecognized value {raw}: {error}",
                std::any::type_name::<T>()
            )
        });
        let written = serde_json::to_string(&parsed).expect("writing the value back must succeed");
        serde_json::from_str(&written).unwrap_or_else(|error| {
            panic!("{} must serialize back to a string: {error}", std::any::type_name::<T>())
        })
    }

    /// A device listing holding one device whose state is `state`.
    fn device_page_json(state: &str) -> String {
        format!(
            r#"{{
                "offset": 0, "limit": 25, "count": 2, "totalCount": 2,
                "data": [
                    {{
                        "id": "00000000-0000-0000-0000-000000000001",
                        "name": "ap-lr", "model": "U6-LR",
                        "macAddress": "00:11:22:33:44:55", "ipAddress": "192.168.1.2",
                        "state": "{state}",
                        "features": ["accessPoint"], "interfaces": ["radios"]
                    }},
                    {{
                        "id": "00000000-0000-0000-0000-000000000002",
                        "name": "switch-8", "model": "USW-8",
                        "macAddress": "00:11:22:33:44:66", "ipAddress": "192.168.1.3",
                        "state": "ONLINE",
                        "features": ["switching"], "interfaces": ["ports"]
                    }}
                ]
            }}"#
        )
    }

    /// Every enum read from the controller must survive a value this build
    /// does not know, and must hand that value back unchanged so `--output
    /// json` stays faithful to what the controller actually said.
    #[test]
    fn unknown_values_round_trip_through_every_api_enum() {
        let checks: &[(&str, fn(&str) -> String)] = &[
            ("DeviceState", round_trip::<DeviceState>),
            ("DeviceFeature", round_trip::<DeviceFeature>),
            ("DeviceInterface", round_trip::<DeviceInterface>),
            ("PortState", round_trip::<PortState>),
            ("PortConnector", round_trip::<PortConnector>),
            ("PoEStandard", round_trip::<PoEStandard>),
            ("PoEState", round_trip::<PoEState>),
            ("WlanStandard", round_trip::<WlanStandard>),
        ];

        for (name, check) in checks {
            assert_eq!(
                check(FUTURE_VALUE),
                FUTURE_VALUE,
                "{name} must hand an unrecognized value back unchanged"
            );
        }
    }

    /// Known values must keep their exact wire spelling, so opening the enums
    /// up cannot quietly rewrite what `--output json` emits.
    #[test]
    fn known_values_keep_their_wire_spelling() {
        let checks: &[(&str, fn(&str) -> String)] = &[
            ("ONLINE", round_trip::<DeviceState>),
            ("accessPoint", round_trip::<DeviceFeature>),
            ("radios", round_trip::<DeviceInterface>),
            ("UP", round_trip::<PortState>),
            ("SFP28", round_trip::<PortConnector>),
            ("802.3bt", round_trip::<PoEStandard>),
            ("LIMITED", round_trip::<PoEState>),
            ("802.11be", round_trip::<WlanStandard>),
        ];

        for (value, check) in checks {
            assert_eq!(check(value), *value, "{value} must round trip unchanged");
        }
    }

    /// One device in an unfamiliar state must not cost the user the whole
    /// listing.
    #[test]
    fn a_device_in_an_unknown_state_does_not_kill_the_listing() {
        let json = device_page_json(FUTURE_VALUE);

        let page: Page<Device> = serde_json::from_str(&json)
            .unwrap_or_else(|error| panic!("the listing must still parse: {error}"));

        assert_eq!(page.data.len(), 2, "every device must survive the parse");
        assert_eq!(page.data[1].name, "switch-8", "the known device must survive");
        let written = serde_json::to_value(&page.data[0]).expect("writing the device back");
        assert_eq!(
            written["state"], FUTURE_VALUE,
            "the unrecognized state must be reported as the controller sent it"
        );
    }

    /// A device the controller reports without a name is a device the user
    /// still needs to see -- and still must not cost them the whole listing.
    #[test]
    fn a_device_without_a_name_does_not_kill_the_listing() {
        let json = r#"{
            "offset": 0, "limit": 25, "count": 1, "totalCount": 1,
            "data": [
                {
                    "id": "00000000-0000-0000-0000-000000000001",
                    "macAddress": "00:11:22:33:44:55", "ipAddress": "192.168.1.2",
                    "state": "PENDING_ADOPTION",
                    "features": [], "interfaces": []
                }
            ]
        }"#;

        let page: Page<Device> = serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("a nameless device must still parse: {error}"));

        assert_eq!(page.data.len(), 1, "the nameless device must survive");
    }

    /// A client type this build has never heard of must not cost the user the
    /// whole client listing, and what is known about it must still come back.
    #[test]
    fn an_unknown_client_type_does_not_kill_the_listing() {
        let json = r#"{
            "offset": 0, "limit": 25, "count": 2, "totalCount": 2,
            "data": [
                {
                    "type": "MESH",
                    "id": "00000000-0000-0000-0000-000000000001",
                    "name": "mesh-node",
                    "ipAddress": "192.168.1.9",
                    "macAddress": "00:11:22:33:44:99",
                    "connectedAt": "2026-07-21T00:00:00Z"
                },
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

        let page: Page<Client> = serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("the client listing must still parse: {error}"));

        assert_eq!(page.data.len(), 2, "every client must survive the parse");
        let written = serde_json::to_value(&page.data[0]).expect("writing the client back");
        assert_eq!(
            written["type"], "MESH",
            "the unrecognized client type must be reported as the controller sent it"
        );
        assert_eq!(
            written["name"], "mesh-node",
            "what is known about an unfamiliar client must still be reported"
        );
    }

    /// A new access tier must not cost the user the client it belongs to.
    #[test]
    fn an_unknown_client_access_type_does_not_kill_the_client() {
        let json = r#"{
            "type": "WIRED",
            "id": "00000000-0000-0000-0000-000000000002",
            "name": "nas",
            "macAddress": "00:11:22:33:44:10",
            "uplinkDeviceId": "00000000-0000-0000-0000-000000000003",
            "access": { "type": "HOTSPOT", "authorized": true }
        }"#;

        let client: Client = serde_json::from_str(json)
            .unwrap_or_else(|error| panic!("the client must still parse: {error}"));

        let written = serde_json::to_value(&client).expect("writing the client back");
        assert_eq!(
            written["access"]["type"], "HOTSPOT",
            "the unrecognized access type must be reported as the controller sent it"
        );
    }

    /// The guardrail for this whole class of bug: an enum read from the
    /// controller that has no unknown-value fallback breaks every command
    /// that touches it the day Ubiquiti ships a new value, so adding one must
    /// fail here rather than in the field.
    ///
    /// An enum is exempt only if it is never deserialized (request bodies we
    /// author ourselves cannot surprise us).
    #[test]
    fn no_api_enum_is_closed_to_unknown_values() {
        const SOURCE: &str = include_str!("models.rs");

        // Only the production half of the file: the test module below holds
        // sample declarations that are text, not types.
        let production = SOURCE.split(TEST_MODULE_ATTRIBUTE).next().unwrap_or(SOURCE);

        let closed: Vec<String> = enum_declarations(production)
            .into_iter()
            .filter(|declaration| declaration.needs_fallback() && !declaration.has_fallback())
            .map(|declaration| declaration.name)
            .collect();

        assert!(
            closed.is_empty(),
            "these enums are read from the UniFi API but reject values this build \
             does not know, which fails the whole response instead of the one odd \
             item: {closed:?}. Declare them with api_enum! (bare string values) or \
             give them an untagged fallback variant (tagged objects)."
        );
    }

    /// One enum declaration found in the source of this file.
    struct EnumDeclaration {
        name: String,
        /// Declared through `api_enum!`, which supplies the fallback itself.
        from_api_enum_macro: bool,
        /// Read from the controller, as opposed to only ever written.
        deserialized: bool,
        /// Carries a variant that swallows anything unrecognized.
        fallback_variant: bool,
    }

    impl EnumDeclaration {
        fn needs_fallback(&self) -> bool {
            self.deserialized
        }

        fn has_fallback(&self) -> bool {
            self.from_api_enum_macro || self.fallback_variant
        }
    }

    /// Find every enum declared in `source`.
    ///
    /// This reads the source text rather than the compiled types because the
    /// point is to catch an enum somebody *adds*, which no amount of testing
    /// the existing types can do.
    ///
    /// # Arguments
    ///
    /// * `source` - The Rust source to scan.
    ///
    /// # Returns
    ///
    /// One entry per enum declaration, in source order.
    fn enum_declarations(source: &str) -> Vec<EnumDeclaration> {
        const MACRO_OPENER: &str = "api_enum! {";

        let lines: Vec<&str> = source.lines().collect();
        let mut declarations = Vec::new();

        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if !trimmed.ends_with('{') {
                continue;
            }
            let Some(rest) = trimmed
                .strip_prefix("pub enum ")
                .or_else(|| trimmed.strip_prefix("enum "))
            else {
                continue;
            };
            let name = rest.trim_end_matches('{').trim().to_string();

            let mut from_api_enum_macro = false;
            let mut deserialized = false;
            for earlier in lines[..index].iter().rev() {
                let earlier = earlier.trim();
                if earlier.starts_with("#[") || earlier.starts_with("///") {
                    deserialized |= earlier.contains("derive") && earlier.contains("Deserialize");
                    continue;
                }
                from_api_enum_macro = earlier.ends_with(MACRO_OPENER);
                break;
            }
            if from_api_enum_macro {
                deserialized = true;
            }

            let fallback_variant = lines[index + 1..]
                .iter()
                .take_while(|body| body.trim() != "}")
                .any(|body| {
                    body.contains("serde(untagged)") || body.contains("serde(other)")
                });

            declarations.push(EnumDeclaration {
                name,
                from_api_enum_macro,
                deserialized,
                fallback_variant,
            });
        }

        declarations
    }

    /// The source scan above is only worth anything if it can actually fail,
    /// so feed it an enum of exactly the shape it is meant to catch.
    #[test]
    fn the_closed_enum_guard_can_fail() {
        let closed_enum_source = "\
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = \"SCREAMING_SNAKE_CASE\")]
pub enum NewlyAddedState {
    Online,
    Offline,
}
";

        let declarations = enum_declarations(closed_enum_source);

        assert_eq!(declarations.len(), 1, "the scan must find the enum at all");
        assert!(
            declarations[0].needs_fallback() && !declarations[0].has_fallback(),
            "a closed enum read from the API must be reported as closed"
        );
    }
}
