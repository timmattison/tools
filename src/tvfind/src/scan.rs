//! Probing the network for televisions.
//!
//! Devices are addressed directly rather than discovered over SSDP or mDNS.
//! Access points routinely filter multicast between bands and between the
//! 2.4 GHz and 5 GHz radios, which silently hides TVs that answer perfectly
//! well when asked directly.

use std::net::Ipv4Addr;
use std::time::Duration;

use reqwest::Client;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::identify::{dial_app_installed, parse_google_tv, parse_roku_device_info, Platform, Tv};

/// Roku External Control Protocol.
pub const ROKU_ECP_PORT: u16 = 8060;
/// Chromecast built-in, present on Google TV sets.
pub const GOOGLETV_CAST_PORT: u16 = 8008;
/// Ports probed on every host, each with the firmware family that answers on
/// it, in the order they are reported.
pub const PROBE_PORTS: &[(u16, Platform)] = &[
    (ROKU_ECP_PORT, Platform::RokuTv),
    (GOOGLETV_CAST_PORT, Platform::GoogleTv),
];

/// DIAL applications that need a display to be of any use.
///
/// Every device with Chromecast built-in answers on [`GOOGLETV_CAST_PORT`] and
/// publishes a manufacturer, a speaker as readily as a television. None of the
/// UPnP document, `/setup/eureka_info`, or `?options=detail` carries a
/// device-type field, so the screen has to be proved another way: a DIAL server
/// only lists an application the device can actually run, and none of these
/// runs without a display. Asking for two rather than one keeps a set that
/// carries neither Netflix nor YouTube from being missed for the wrong reason.
pub const SCREEN_APPS: &[&str] = &["Netflix", "YouTube"];

/// How long to wait for a TCP handshake before treating a host as closed.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(1200);
/// How long to wait for a discovery document once connected.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(4);

/// Whether a TCP connection to `ip:port` completes within [`CONNECT_TIMEOUT`].
pub async fn is_port_open(ip: Ipv4Addr, port: u16) -> bool {
    matches!(
        timeout(CONNECT_TIMEOUT, TcpStream::connect((ip, port))).await,
        Ok(Ok(_))
    )
}

/// Body of a successful `GET`, or `None` for any transport or status failure.
async fn get_text(client: &Client, url: &str) -> Option<String> {
    let response = client.get(url).timeout(HTTP_TIMEOUT).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.text().await.ok()
}

/// Fetch and parse a Roku ECP device-info document from `base_url`.
///
/// `base_url` is the scheme and authority only, e.g. `http://192.168.0.119:8060`.
pub async fn fetch_roku(client: &Client, base_url: &str, ip: Ipv4Addr) -> Option<Tv> {
    let xml = get_text(client, &format!("{base_url}/query/device-info")).await?;
    parse_roku_device_info(ip, &xml)
}

/// Whether the DIAL server at `base_url` offers any of [`SCREEN_APPS`].
///
/// Asking stops at the first application the device confirms.
async fn has_screen(client: &Client, base_url: &str) -> bool {
    for app in SCREEN_APPS {
        let Some(body) = get_text(client, &format!("{base_url}/apps/{app}")).await else {
            continue;
        };
        if dial_app_installed(&body, app) {
            return true;
        }
    }
    false
}

/// Fetch and parse a Google TV UPnP description from `base_url`.
///
/// The device must also pass the DIAL screen test, because every speaker with
/// Chromecast built-in publishes the same UPnP document a television does.
///
/// `base_url` is the scheme and authority only, e.g. `http://192.168.1.165:8008`.
pub async fn fetch_google_tv(client: &Client, base_url: &str, ip: Ipv4Addr) -> Option<Tv> {
    let desc = get_text(client, &format!("{base_url}/ssdp/device-desc.xml")).await?;
    // The cast endpoint only supplies a nicer name, so its failure is survivable.
    let eureka = get_text(client, &format!("{base_url}/setup/eureka_info")).await;

    let tv = parse_google_tv(ip, &desc, eureka.as_deref())?;
    has_screen(client, base_url).await.then_some(tv)
}

/// What one probe of one host on one port found.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Probe {
    /// Address that was probed.
    pub ip: Ipv4Addr,
    /// Whether the TCP handshake completed.
    pub answered: bool,
    /// The television, if the host proved that it is one.
    pub tv: Option<Tv>,
}

/// Probe `ip` on `port` and report what the exchange found.
///
/// `platform` names the firmware family that answers on `port`, so the map from
/// a port to its family stays in one place.
///
/// A host that completes the TCP handshake answered, even if it is not a
/// television. A Roku streaming player answers the ECP port and a speaker with
/// Chromecast built-in answers the cast port. Both devices have power, so
/// neither belongs in the powered-off report.
pub async fn probe(client: &Client, ip: Ipv4Addr, port: u16, platform: Platform) -> Probe {
    if !is_port_open(ip, port).await {
        return Probe {
            ip,
            answered: false,
            tv: None,
        };
    }

    let base_url = format!("http://{ip}:{port}");
    let tv = match platform {
        Platform::RokuTv => fetch_roku(client, &base_url, ip).await,
        Platform::GoogleTv => fetch_google_tv(client, &base_url, ip).await,
    };

    Probe {
        ip,
        answered: true,
        tv,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROKU_DEVICE_INFO: &str = r"<device-info>
<vendor-name>TCL</vendor-name>
<model-name>43S435</model-name>
<is-tv>true</is-tv>
<user-device-name>Office - top</user-device-name>
<software-version>15.0.4</software-version>
</device-info>";

    const GOOGLE_TV_DESC: &str = r"<root>
  <device>
    <friendlyName>Living Room</friendlyName>
    <manufacturer>TCL</manufacturer>
    <modelName>Smart TV Pro</modelName>
  </device>
</root>";

    fn ip() -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 0, 119)
    }

    /// Bind an ephemeral port so concurrent runs of this suite never collide.
    fn ephemeral_port() -> (std::net::TcpListener, u16) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("should bind");
        let port = listener
            .local_addr()
            .expect("should have an address")
            .port();
        (listener, port)
    }

    #[tokio::test]
    async fn sees_a_port_that_is_being_listened_on() {
        let (_listener, port) = ephemeral_port();

        assert!(is_port_open(Ipv4Addr::LOCALHOST, port).await);
    }

    #[tokio::test]
    async fn sees_a_port_with_nothing_behind_it_as_closed() {
        let (listener, port) = ephemeral_port();
        drop(listener);

        assert!(!is_port_open(Ipv4Addr::LOCALHOST, port).await);
    }

    #[tokio::test]
    async fn fetches_and_identifies_a_roku_tv() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/query/device-info")
            .with_status(200)
            .with_body(ROKU_DEVICE_INFO)
            .create_async()
            .await;

        let tv = fetch_roku(&Client::new(), &server.url(), ip())
            .await
            .expect("should identify the TV");

        mock.assert_async().await;
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.name, "Office - top");
        assert_eq!(tv.ip, ip());
    }

    #[tokio::test]
    async fn treats_a_non_roku_responder_on_the_ecp_port_as_no_tv() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/query/device-info")
            .with_status(404)
            .create_async()
            .await;

        assert!(fetch_roku(&Client::new(), &server.url(), ip())
            .await
            .is_none());
        mock.assert_async().await;
    }

    /// `/apps/Netflix` as a TCL Google TV answers it.
    const NETFLIX_DIAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<service xmlns="urn:dial-multiscreen-org:schemas:dial">
  <name>Netflix</name>
  <options allowStop="true"/>
  <state>stopped</state>
</service>"#;

    /// Answer the DIAL screen test the way a device with a display does.
    async fn mock_a_screen(server: &mut mockito::Server) -> mockito::Mock {
        server
            .mock("GET", "/apps/Netflix")
            .with_status(200)
            .with_body(NETFLIX_DIAL)
            .create_async()
            .await
    }

    #[tokio::test]
    async fn fetches_and_identifies_a_google_tv() {
        let mut server = mockito::Server::new_async().await;
        let desc = server
            .mock("GET", "/ssdp/device-desc.xml")
            .with_status(200)
            .with_body(GOOGLE_TV_DESC)
            .create_async()
            .await;
        let eureka = server
            .mock("GET", "/setup/eureka_info")
            .with_status(200)
            .with_body(r#"{"name":"Living Room","cast_build_revision":"3.72.446070"}"#)
            .create_async()
            .await;
        let _screen = mock_a_screen(&mut server).await;

        let tv = fetch_google_tv(&Client::new(), &server.url(), ip())
            .await
            .expect("should identify the TV");

        desc.assert_async().await;
        eureka.assert_async().await;
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.model, "Smart TV Pro");
        assert_eq!(tv.software, "3.72.446070");
    }

    #[tokio::test]
    async fn identifies_a_google_tv_whose_cast_endpoint_refuses() {
        let mut server = mockito::Server::new_async().await;
        let desc = server
            .mock("GET", "/ssdp/device-desc.xml")
            .with_status(200)
            .with_body(GOOGLE_TV_DESC)
            .create_async()
            .await;
        let eureka = server
            .mock("GET", "/setup/eureka_info")
            .with_status(500)
            .create_async()
            .await;
        let _screen = mock_a_screen(&mut server).await;

        let tv = fetch_google_tv(&Client::new(), &server.url(), ip())
            .await
            .expect("UPnP alone should be enough");

        desc.assert_async().await;
        eureka.assert_async().await;
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.name, "Living Room");
    }

    #[tokio::test]
    async fn treats_a_cast_device_with_no_video_application_as_no_tv() {
        // A speaker with Chromecast built-in publishes the same UPnP document a
        // TV does. Only its refusal to run a video app separates the two.
        let mut server = mockito::Server::new_async().await;
        let _desc = server
            .mock("GET", "/ssdp/device-desc.xml")
            .with_status(200)
            .with_body(
                GOOGLE_TV_DESC
                    .replace("Smart TV Pro", "Google Home")
                    .as_str(),
            )
            .create_async()
            .await;
        let _eureka = server
            .mock("GET", "/setup/eureka_info")
            .with_status(200)
            .with_body(r#"{"name":"Kitchen speaker"}"#)
            .create_async()
            .await;
        let _no_apps = server
            .mock("GET", mockito::Matcher::Regex(r"^/apps/.*$".to_owned()))
            .with_status(404)
            .expect_at_least(1)
            .create_async()
            .await;

        assert!(fetch_google_tv(&Client::new(), &server.url(), ip())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn asks_a_second_video_application_when_the_first_is_absent() {
        // Not every set carries Netflix, so one refusal must not end the test.
        let mut server = mockito::Server::new_async().await;
        let _desc = server
            .mock("GET", "/ssdp/device-desc.xml")
            .with_status(200)
            .with_body(GOOGLE_TV_DESC)
            .create_async()
            .await;
        let _eureka = server
            .mock("GET", "/setup/eureka_info")
            .with_status(404)
            .create_async()
            .await;
        let netflix = server
            .mock("GET", "/apps/Netflix")
            .with_status(404)
            .create_async()
            .await;
        let youtube = server
            .mock("GET", "/apps/YouTube")
            .with_status(200)
            .with_body(NETFLIX_DIAL.replace("Netflix", "YouTube").as_str())
            .create_async()
            .await;

        let tv = fetch_google_tv(&Client::new(), &server.url(), ip())
            .await
            .expect("a second video app should still prove a screen");

        netflix.assert_async().await;
        youtube.assert_async().await;
        assert_eq!(tv.vendor, "TCL");
    }

    /// A Roku streaming player answers ECP with the same document a TV does.
    /// Only `is-tv` separates the two.
    const ROKU_STREAMING_STICK: &str = r"<device-info>
<vendor-name>Roku</vendor-name>
<model-name>Roku Express</model-name>
<is-tv>false</is-tv>
<friendly-device-name>Roku Express</friendly-device-name>
<software-version>15.2.4</software-version>
</device-info>";

    /// The port a mock server listens on, for a probe of `127.0.0.1`.
    fn mock_port(server: &mockito::Server) -> u16 {
        server.socket_address().port()
    }

    #[tokio::test]
    async fn records_a_roku_streaming_player_as_a_host_that_answered() {
        // The player is not a television, but it has power. A host with power
        // must never appear in the powered-off report.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/query/device-info")
            .with_status(200)
            .with_body(ROKU_STREAMING_STICK)
            .create_async()
            .await;
        let port = mock_port(&server);

        let found = probe(&Client::new(), Ipv4Addr::LOCALHOST, port, Platform::RokuTv).await;

        mock.assert_async().await;
        assert!(found.answered, "the TCP handshake completed");
        assert_eq!(found.tv, None, "a streaming player is not a television");
        assert_eq!(found.ip, Ipv4Addr::LOCALHOST);
    }

    #[tokio::test]
    async fn records_a_roku_television_as_a_host_that_answered_with_a_tv() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/query/device-info")
            .with_status(200)
            .with_body(ROKU_DEVICE_INFO)
            .create_async()
            .await;
        let port = mock_port(&server);

        let found = probe(&Client::new(), Ipv4Addr::LOCALHOST, port, Platform::RokuTv).await;

        assert!(found.answered);
        let tv = found.tv.expect("should identify the TV");
        assert_eq!(tv.vendor, "TCL");
        assert_eq!(tv.platform, Platform::RokuTv);
    }

    #[tokio::test]
    async fn records_a_cast_speaker_as_a_host_that_answered() {
        // A speaker with Chromecast built-in answers the cast port and fails
        // the screen test. It has power, so it did answer.
        let mut server = mockito::Server::new_async().await;
        let _desc = server
            .mock("GET", "/ssdp/device-desc.xml")
            .with_status(200)
            .with_body(
                GOOGLE_TV_DESC
                    .replace("Smart TV Pro", "Google Home")
                    .as_str(),
            )
            .create_async()
            .await;
        let _eureka = server
            .mock("GET", "/setup/eureka_info")
            .with_status(200)
            .with_body(r#"{"name":"Kitchen speaker"}"#)
            .create_async()
            .await;
        let _no_apps = server
            .mock("GET", mockito::Matcher::Regex(r"^/apps/.*$".to_owned()))
            .with_status(404)
            .expect_at_least(1)
            .create_async()
            .await;
        let port = mock_port(&server);

        let found = probe(
            &Client::new(),
            Ipv4Addr::LOCALHOST,
            port,
            Platform::GoogleTv,
        )
        .await;

        assert!(found.answered, "the TCP handshake completed");
        assert_eq!(found.tv, None, "a speaker is not a television");
    }

    #[tokio::test]
    async fn records_a_closed_port_as_a_host_that_did_not_answer() {
        let (listener, port) = ephemeral_port();
        drop(listener);

        let found = probe(&Client::new(), Ipv4Addr::LOCALHOST, port, Platform::RokuTv).await;

        assert!(!found.answered);
        assert_eq!(found.tv, None);
        assert_eq!(found.ip, Ipv4Addr::LOCALHOST);
    }
}
