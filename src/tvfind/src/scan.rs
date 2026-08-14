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

/// Fetch and parse a Google TV UPnP description from `base_url`.
///
/// `base_url` is the scheme and authority only, e.g. `http://192.168.1.165:8008`.
pub async fn fetch_google_tv(client: &Client, base_url: &str, ip: Ipv4Addr) -> Option<Tv> {
    let desc = get_text(client, &format!("{base_url}/ssdp/device-desc.xml")).await?;
    // The cast endpoint only supplies a nicer name, so its failure is survivable.
    let eureka = get_text(client, &format!("{base_url}/setup/eureka_info")).await;

    parse_google_tv(ip, &desc, eureka.as_deref())
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
            .with_body(GOOGLE_TV_DESC.replace("Smart TV Pro", "Google Home").as_str())
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
}
