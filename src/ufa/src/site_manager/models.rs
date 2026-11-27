use serde::{Deserialize, Serialize};

/// One cloud host, in the shape of the `Host` schema of the official Site
/// Manager OpenAPI document (`https://developer.ui.com/site-manager/v1.0.0/openapi.json`).
///
/// A time field is kept as the API sent it. The spec sends an empty string
/// for a time that it does not know, and a date type refuses that string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    pub id: String,
    pub hardware_id: String,
    #[serde(rename = "type")]
    pub host_type: String,
    pub ip_address: Option<String>,
    pub is_blocked: bool,
    /// When the host last changed its connection state, as the API sent it:
    /// an RFC 3339 time, or an empty string.
    pub last_connection_state_change: Option<String>,
    /// When the host last made a backup, as the API sent it: an RFC 3339
    /// time, or an empty string.
    pub latest_backup_time: Option<String>,
    pub owner: bool,
    /// When the host registered, as the API sent it: an RFC 3339 time, or an
    /// empty string.
    pub registration_time: Option<String>,
    pub reported_state: Option<ReportedState>,
    pub user_data: Option<UserData>,
    /// Each other field the API sent, kept verbatim, so `--output json`
    /// writes the host back with each field it had.
    #[serde(flatten)]
    pub other_fields: serde_json::Map<String, serde_json::Value>,
}

/// The snake_case key of the firmware version, as the `network-server`
/// example of the spec spells it.
const SNAKE_FIRMWARE_KEY: &str = "firmware_version";

/// The key of the software version that a host reports beside a null
/// `firmware_version`, in the `network-server` example of the spec.
const VERSION_KEY: &str = "version";

/// The state that a host reports about itself.
///
/// The spec gives `reportedState` no schema, so it promises no field in it.
/// Each field is thus optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedState {
    /// The controllers of the host, as raw JSON. The spec gives no shape for
    /// them, and nothing in this crate reads one.
    pub controllers: Option<Vec<serde_json::Value>>,
    /// The firmware version under the key `firmwareVersion`. A host that
    /// spells the key `firmware_version` keeps it in `other_fields`, and
    /// [`ReportedState::firmware`] reads both.
    pub firmware_version: Option<String>,
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
    /// Each other field the host reported, kept verbatim.
    #[serde(flatten)]
    pub other_fields: serde_json::Map<String, serde_json::Value>,
}

impl ReportedState {
    /// The firmware version that the host reported.
    ///
    /// # Returns
    ///
    /// The text of `firmwareVersion` when the host set it, else the text of
    /// `firmware_version`. The text of `version` when the host set neither
    /// firmware key, or set each one to null. `None` when the host set none
    /// of the three.
    pub fn firmware(&self) -> Option<&str> {
        self.firmware_version.as_deref().or_else(|| {
            [SNAKE_FIRMWARE_KEY, VERSION_KEY].into_iter().find_map(|key| {
                self.other_fields
                    .get(key)
                    .and_then(serde_json::Value::as_str)
            })
        })
    }
}

/// What the account keeps about a host: its name and its note, and the role,
/// the permissions and the members that the spec example shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserData {
    pub name: Option<String>,
    pub note: Option<String>,
    /// Each other field the API sent, kept verbatim.
    #[serde(flatten)]
    pub other_fields: serde_json::Map<String, serde_json::Value>,
}

/// One page of the answer of `GET /v1/hosts`, in the shape that the official
/// Site Manager OpenAPI document gives it.
///
/// The answer also carries `httpStatusCode` and `traceId`. Nothing reads them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostsResponse {
    /// The hosts of this page.
    pub data: Vec<Host>,
    /// The token that names the next page. It is absent, or empty, on the
    /// last page.
    pub next_token: Option<String>,
}

/// The answer of `GET /v1/hosts/{id}`, in the shape that the official Site
/// Manager OpenAPI document gives it: the host sits in the `data` key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostResponse {
    /// The host that the id names.
    pub data: Host,
}
