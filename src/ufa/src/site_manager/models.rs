use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub id: String,
    pub hardware_id: String,
    #[serde(rename = "type")]
    pub host_type: String,
    pub ip_address: Option<String>,
    pub is_blocked: bool,
    pub last_connection_state_change: Option<DateTime<Utc>>,
    pub latest_backup_time: Option<DateTime<Utc>>,
    pub owner: bool,
    pub registration_time: Option<DateTime<Utc>>,
    pub reported_state: Option<ReportedState>,
    pub user_data: Option<UserData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedState {
    pub controllers: Option<Vec<Controller>>,
    pub firmware_version: Option<String>,
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub is_running: bool,
    pub name: String,
    pub port: u16,
    pub release_channel: Option<String>,
    pub status: String,
    #[serde(rename = "type")]
    pub controller_type: String,
    pub ui_version: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserData {
    pub name: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsResponse {
    pub hosts: Vec<Host>,
    pub total: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorResponse {
    pub message: String,
    pub code: Option<String>,
}