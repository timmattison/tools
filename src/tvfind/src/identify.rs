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

impl Tv {
    /// The cells describing this TV, in table-column order.
    ///
    /// A field the device left blank renders as a dash so columns stay legible.
    #[must_use]
    pub fn display_row(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Text of the first `<tag>…</tag>` in a flat XML document.
///
/// Discovery documents are machine-generated and single-level, so splitting on
/// the delimiters is both sufficient and cheaper than a full parser. Splitting
/// rather than slicing also keeps every boundary on a `char`, so multi-byte
/// device names survive intact.
fn xml_tag(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");

    xml.split_once(open.as_str())
        .and_then(|(_, rest)| rest.split_once(close.as_str()))
        .map(|(value, _)| value.trim().to_owned())
        .unwrap_or_default()
}

/// Parse a Roku ECP `/query/device-info` response.
///
/// Returns `None` when the payload is not a Roku device-info document.
#[must_use]
pub fn parse_roku_device_info(ip: Ipv4Addr, xml: &str) -> Option<Tv> {
    if !xml.contains("<device-info>") {
        return None;
    }

    let vendor = xml_tag(xml, "vendor-name");
    if vendor.is_empty() {
        return None;
    }

    // A set the owner has named reports it in `user-device-name`; one still on
    // factory defaults leaves that empty and only fills `friendly-device-name`.
    let mut name = xml_tag(xml, "user-device-name");
    if name.is_empty() {
        name = xml_tag(xml, "friendly-device-name");
    }

    Some(Tv {
        ip,
        platform: Platform::RokuTv,
        vendor,
        model: xml_tag(xml, "model-name"),
        name,
        software: xml_tag(xml, "software-version"),
    })
}

/// Parse a Google TV UPnP description, enriched with `/setup/eureka_info`.
///
/// `eureka_json` is best-effort: the UPnP document alone is enough to identify
/// the device, and the cast payload only supplies a better display name.
///
/// Returns `None` when the payload is not a UPnP device description.
#[must_use]
pub fn parse_google_tv(ip: Ipv4Addr, desc_xml: &str, eureka_json: Option<&str>) -> Option<Tv> {
    let vendor = xml_tag(desc_xml, "manufacturer");
    if vendor.is_empty() {
        return None;
    }

    // Renaming a set updates its cast name but can leave the UPnP document
    // holding the original, so the cast name wins when both are present.
    let cast: Option<serde_json::Value> =
        eureka_json.and_then(|json| serde_json::from_str(json).ok());
    let cast_field = |key: &str| -> String {
        cast.as_ref()
            .and_then(|value| value.get(key))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned()
    };

    let mut name = cast_field("name");
    if name.is_empty() {
        name = xml_tag(desc_xml, "friendlyName");
    }

    Some(Tv {
        ip,
        platform: Platform::GoogleTv,
        vendor,
        model: xml_tag(desc_xml, "modelName"),
        name,
        software: cast_field("cast_build_revision"),
    })
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

    /// Verbatim `/ssdp/device-desc.xml` from a TCL Google TV.
    const GOOGLE_TV_DESC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <URLBase>http://192.168.1.165:8008</URLBase>
  <device>
    <deviceType>urn:dial-multiscreen-org:device:dial:1</deviceType>
    <friendlyName>Living Room</friendlyName>
    <manufacturer>TCL</manufacturer>
    <modelName>Smart TV Pro</modelName>
    <UDN>uuid:1bee973d-e7a0-9858-38cd-a2a5a11119c1</UDN>
  </device>
</root>"#;

    /// Trimmed `/setup/eureka_info` from the same set.
    const GOOGLE_TV_EUREKA: &str =
        r#"{"build_version":"446070","cast_build_revision":"3.72.446070","name":"Living Room"}"#;

    fn google_ip() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 1, 165)
    }

    #[test]
    fn reads_vendor_model_and_cast_build_from_a_google_tv() {
        let tv = parse_google_tv(google_ip(), GOOGLE_TV_DESC, Some(GOOGLE_TV_EUREKA))
            .expect("should identify a Google TV");

        assert_eq!(tv.ip, google_ip());
        assert_eq!(tv.platform, Platform::GoogleTv);
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.model, "Smart TV Pro");
        assert_eq!(tv.name, "Living Room");
        assert_eq!(tv.software, "3.72.446070");
    }

    #[test]
    fn prefers_the_cast_name_over_the_upnp_friendly_name() {
        // A renamed set updates its cast name while UPnP keeps the old one.
        let eureka = r#"{"name":"Den","cast_build_revision":"3.72.446070"}"#;

        let tv = parse_google_tv(google_ip(), GOOGLE_TV_DESC, Some(eureka))
            .expect("should identify a Google TV");

        assert_eq!(tv.name, "Den");
    }

    #[test]
    fn identifies_a_google_tv_even_when_the_cast_endpoint_is_unavailable() {
        let tv = parse_google_tv(google_ip(), GOOGLE_TV_DESC, None)
            .expect("UPnP alone should be enough to identify");

        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.name, "Living Room");
        assert_eq!(tv.software, "");
    }

    #[test]
    fn survives_a_malformed_cast_payload() {
        let tv = parse_google_tv(google_ip(), GOOGLE_TV_DESC, Some("not json at all"))
            .expect("a bad cast payload must not lose the UPnP identification");

        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.name, "Living Room");
    }

    #[test]
    fn rejects_a_payload_that_names_no_manufacturer() {
        assert!(parse_google_tv(google_ip(), "<html><body>404</body></html>", None).is_none());
    }

    #[test]
    fn renders_every_field_of_a_roku_tv_in_column_order() {
        let tv = parse_roku_device_info(ip(), ROKU_DEVICE_INFO).expect("should identify");

        assert_eq!(
            tv.display_row(),
            [
                "192.168.0.119",
                "Office - top",
                "TCL",
                "43S435",
                "Roku TV",
                "15.0.4"
            ]
            .map(String::from)
        );
    }

    #[test]
    fn labels_a_google_tv_row_with_its_platform() {
        let tv = parse_google_tv(google_ip(), GOOGLE_TV_DESC, None).expect("should identify");
        let row = tv.display_row();

        assert_eq!(row.get(4).map(String::as_str), Some("Google TV"));
    }

    #[test]
    fn renders_a_field_the_device_left_blank_as_a_dash() {
        let xml = ROKU_DEVICE_INFO.replace(
            "<software-version>15.0.4</software-version>",
            "<software-version></software-version>",
        );
        let tv = parse_roku_device_info(ip(), &xml).expect("should identify");
        let row = tv.display_row();

        assert_eq!(row.last().map(String::as_str), Some("-"));
    }
}
