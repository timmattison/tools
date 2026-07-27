//! Turn a TV's own discovery documents into an identified device.
//!
//! Two firmware families cover essentially every consumer smart TV that
//! answers on the LAN, and each publishes an authoritative vendor string:
//!
//! * **Roku TV** — Roku's External Control Protocol on `tcp/8060`.
//!   `GET /query/device-info` returns XML whose `<vendor-name>` is set by the
//!   panel manufacturer (`TCL`, `Hisense`, `Roku`, ...).
//! * **Google TV / Chromecast built-in** — `tcp/8008`.
//!   `GET /ssdp/device-desc.xml` returns UPnP XML carrying `<manufacturer>`,
//!   and `GET /setup/eureka_info` returns the name the user assigned.

use std::net::Ipv4Addr;

/// Firmware family a TV was identified through.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Platform {
    /// Identified over Roku ECP on port 8060.
    RokuTv,
    /// Identified over Chromecast built-in on port 8008.
    GoogleTv,
}

impl Platform {
    /// Human-readable name for table output.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::RokuTv => "Roku TV",
            Self::GoogleTv => "Google TV",
        }
    }
}

/// A television positively identified from its own discovery documents.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Tv {
    /// Address the device answered on.
    pub ip: Ipv4Addr,
    /// Which firmware family identified it.
    pub platform: Platform,
    /// Manufacturer as reported by the device itself.
    pub vendor: String,
    /// Model designation, e.g. `43S435`.
    pub model: String,
    /// Name the user assigned, e.g. `Living Room`.
    pub name: String,
    /// Firmware or cast build version.
    pub software: String,
}

/// Text of the first `<tag>…</tag>` in a flat XML document.
fn xml_tag(xml: &str, tag: &str) -> String {
    let _ = (xml, tag);
    String::new()
}

/// Parse a Roku ECP `/query/device-info` response.
///
/// Returns `None` when the payload is not a Roku device-info document.
#[must_use]
pub fn parse_roku_device_info(ip: Ipv4Addr, xml: &str) -> Option<Tv> {
    let _ = (ip, xml);
    None
}

/// Parse a Google TV UPnP description, enriched with `/setup/eureka_info`.
///
/// `eureka_json` is best-effort: the UPnP document alone is enough to identify
/// the device, and the cast payload only supplies a better display name.
///
/// Returns `None` when the payload is not a UPnP device description.
#[must_use]
pub fn parse_google_tv(ip: Ipv4Addr, desc_xml: &str, eureka_json: Option<&str>) -> Option<Tv> {
    let _ = (ip, desc_xml, eureka_json);
    None
}
