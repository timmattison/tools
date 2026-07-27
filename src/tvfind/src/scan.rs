//! Probing the network for televisions.
//!
//! Devices are addressed directly rather than discovered over SSDP or mDNS.
//! Access points routinely filter multicast between bands and between the
//! 2.4 GHz and 5 GHz radios, which silently hides TVs that answer perfectly
//! well when asked directly.

use std::net::Ipv4Addr;
use std::time::Duration;

use reqwest::Client;

use crate::identify::{parse_google_tv, parse_roku_device_info, Tv};

/// Roku External Control Protocol.
pub const ROKU_ECP_PORT: u16 = 8060;
/// Chromecast built-in, present on Google TV sets.
pub const GOOGLETV_CAST_PORT: u16 = 8008;
/// Ports probed on every host, in the order they are reported.
pub const PROBE_PORTS: &[u16] = &[ROKU_ECP_PORT, GOOGLETV_CAST_PORT];

/// How long to wait for a TCP handshake before treating a host as closed.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(1200);
/// How long to wait for a discovery document once connected.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(4);

/// Whether a TCP connection to `ip:port` completes within [`CONNECT_TIMEOUT`].
pub async fn is_port_open(ip: Ipv4Addr, port: u16) -> bool {
    let _ = (ip, port);
    false
}

/// Fetch and parse a Roku ECP device-info document from `base_url`.
///
/// `base_url` is the scheme and authority only, e.g. `http://192.168.0.119:8060`.
pub async fn fetch_roku(client: &Client, base_url: &str, ip: Ipv4Addr) -> Option<Tv> {
    let _ = (client, base_url, ip);
    let _ = parse_roku_device_info;
    None
}

/// Fetch and parse a Google TV UPnP description from `base_url`.
///
/// `base_url` is the scheme and authority only, e.g. `http://192.168.1.165:8008`.
pub async fn fetch_google_tv(client: &Client, base_url: &str, ip: Ipv4Addr) -> Option<Tv> {
    let _ = (client, base_url, ip);
    let _ = parse_google_tv;
    None
}
