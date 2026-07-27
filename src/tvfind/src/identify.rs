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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim `/query/device-info` response from a TCL 43S435 Roku TV.
    const ROKU_DEVICE_INFO: &str = r"<device-info>
<udn>29badd40-cd5a-50ab-b7c8-1b1cd0834f2c</udn>
<serial-number>X000002403FJ</serial-number>
<device-id>S02ST0C403FJ</device-id>
<vendor-name>TCL</vendor-name>
<model-name>43S435</model-name>
<model-number>C134X</model-number>
<is-tv>true</is-tv>
<screen-size>43</screen-size>
<friendly-device-name>Office - top</friendly-device-name>
<user-device-name>Office - top</user-device-name>
<user-device-location>Office</user-device-location>
<software-version>15.0.4</software-version>
</device-info>";

    fn ip() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 0, 119)
    }

    #[test]
    fn reads_vendor_model_name_and_version_from_a_roku_tv() {
        let tv = parse_roku_device_info(ip(), ROKU_DEVICE_INFO).expect("should identify a Roku TV");

        assert_eq!(tv.ip, ip());
        assert_eq!(tv.platform, Platform::RokuTv);
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.model, "43S435");
        assert_eq!(tv.name, "Office - top");
        assert_eq!(tv.software, "15.0.4");
    }

    #[test]
    fn prefers_the_user_assigned_name_over_the_friendly_name() {
        let xml = ROKU_DEVICE_INFO.replace(
            "<friendly-device-name>Office - top</friendly-device-name>",
            "<friendly-device-name>Roku Express</friendly-device-name>",
        );

        let tv = parse_roku_device_info(ip(), &xml).expect("should identify a Roku TV");

        assert_eq!(tv.name, "Office - top");
    }

    #[test]
    fn falls_back_to_the_friendly_name_when_no_name_was_assigned() {
        let xml = ROKU_DEVICE_INFO.replace(
            "<user-device-name>Office - top</user-device-name>",
            "<user-device-name></user-device-name>",
        );

        let tv = parse_roku_device_info(ip(), &xml).expect("should identify a Roku TV");

        assert_eq!(tv.name, "Office - top");
    }

    #[test]
    fn keeps_multibyte_device_names_intact() {
        let xml = ROKU_DEVICE_INFO.replace(
            "<user-device-name>Office - top</user-device-name>",
            "<user-device-name>リビング 🎉</user-device-name>",
        );

        let tv = parse_roku_device_info(ip(), &xml).expect("should identify a Roku TV");

        assert_eq!(tv.name, "リビング 🎉");
    }

    #[test]
    fn rejects_a_payload_that_is_not_a_roku_device_info_document() {
        assert!(parse_roku_device_info(ip(), "<html><body>404</body></html>").is_none());
    }
}
