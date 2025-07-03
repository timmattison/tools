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
    pub frequency_ghz: String,
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
    #[serde(rename = "authorizedGuestLimit", skip_serializing_if = "Option::is_none")]
    pub authorized_guest_limit: Option<u64>,
    #[serde(rename = "timeLimitMinutes")]
    pub time_limit_minutes: u64,
    #[serde(rename = "dataUsageLimitMBytes", skip_serializing_if = "Option::is_none")]
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
        #[serde(rename = "dataUsageLimitMBytes", skip_serializing_if = "Option::is_none")]
        data_usage_limit_mbytes: Option<u64>,
        #[serde(rename = "rxRateLimitKbps", skip_serializing_if = "Option::is_none")]
        rx_rate_limit_kbps: Option<u64>,
        #[serde(rename = "txRateLimitKbps", skip_serializing_if = "Option::is_none")]
        tx_rate_limit_kbps: Option<u64>,
    },
    #[serde(rename = "UNAUTHORIZE_GUEST_ACCESS")]
    UnauthorizeGuestAccess,
}