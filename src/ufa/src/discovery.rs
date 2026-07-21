use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub struct DiscoveredController {
    pub ip: String,
    pub port: u16,
    pub name: Option<String>,
    pub is_verified: bool,
}

impl DiscoveredController {
    pub fn url(&self) -> String {
        format!("https://{}:{}", self.ip, self.port)
    }
}

/// Discover UniFi controllers on the local network
pub async fn discover_controllers() -> Result<Vec<DiscoveredController>> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid spinner template"),
    );
    spinner.enable_steady_tick(Duration::from_millis(80));

    let mut controllers = Vec::new();

    // Try mDNS discovery
    spinner.set_message("mDNS: browsing for _unifi._tcp.local. ...");
    if let Ok(mdns_controllers) = discover_via_mdns().await {
        controllers.extend(mdns_controllers);
    }

    // Try common IPs and ports in parallel
    let common_targets: Vec<(&str, u16)> = vec![
        ("192.168.1.1", 443),
        ("192.168.1.1", 8443),
        ("192.168.0.1", 443),
        ("192.168.0.1", 8443),
        ("unifi", 443),
        ("unifi", 8443),
        ("unifi.local", 443),
        ("unifi.local", 8443),
    ];

    spinner.set_message(format!(
        "Probing {} common addresses ...",
        common_targets.len()
    ));

    let futures: Vec<_> = common_targets
        .into_iter()
        .map(|(host, port)| validate_controller(host, port))
        .collect();

    let results = futures::future::join_all(futures).await;
    for controller in results.into_iter().flatten() {
        controllers.push(controller);
    }

    spinner.finish_and_clear();

    // Deduplicate by IP
    let mut seen = HashSet::new();
    controllers.retain(|c| seen.insert(c.ip.clone()));

    Ok(controllers)
}

/// Discover controllers via mDNS
async fn discover_via_mdns() -> Result<Vec<DiscoveredController>> {
    let mdns = ServiceDaemon::new()?;
    let mut controllers = Vec::new();

    // Browse for UniFi services
    let receiver = mdns.browse("_unifi._tcp.local.")?;

    // Collect responses for a short time
    let browse_duration = Duration::from_secs(2);
    let _ = timeout(browse_duration, async {
        while let Ok(event) = receiver.recv_async().await {
            if let ServiceEvent::ServiceResolved(info) = event {
                for addr in info.get_addresses() {
                    let controller = DiscoveredController {
                        ip: addr.to_string(),
                        port: info.get_port(),
                        name: Some(info.get_fullname().to_string()),
                        is_verified: false,
                    };
                    controllers.push(controller);
                }
            }
        }
    })
    .await;

    // Verify each discovered controller
    let mut verified_controllers = Vec::new();
    for mut controller in controllers {
        if let Ok(verified) = validate_controller(&controller.ip, controller.port).await {
            controller.is_verified = true;
            controller.name = controller.name.or(verified.name);
            verified_controllers.push(controller);
        }
    }

    Ok(verified_controllers)
}

/// Validate that a given host:port is a UniFi controller
pub async fn validate_controller(host: &str, port: u16) -> Result<DiscoveredController> {
    let url = format!("https://{}:{}", host, port);

    // Create a client that accepts self-signed certificates
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(3))
        .build()?;

    // Try to fetch the root page
    let response = client.get(&url).send().await?;

    // Check if it's a UniFi controller by looking at the response
    let text = response.text().await?;

    // Look for UniFi-specific markers
    if text.contains("window.UNIFI_")
        || text.contains("UniFi")
        || text.contains("ui-icon")
        || text.contains("/api/login")
    {
        // Try to resolve the IP if we were given a hostname
        let ip = if host.parse::<std::net::IpAddr>().is_ok() {
            host.to_string()
        } else {
            // Resolve hostname to IP
            use std::net::ToSocketAddrs;
            let addr = format!("{}:{}", host, port);
            addr.to_socket_addrs()?
                .next()
                .map(|s| s.ip().to_string())
                .unwrap_or_else(|| host.to_string())
        };

        Ok(DiscoveredController {
            ip,
            port,
            name: None,
            is_verified: true,
        })
    } else {
        anyhow::bail!("Not a UniFi controller")
    }
}

/// What a probe of a host says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// The host answers the integration API: it is a controller.
    Controller,
    /// Whatever is listening there, it is not a controller.
    NotController,
}

/// Decide whether the answer to an integration-API probe came from a UniFi
/// controller.
///
/// # Arguments
///
/// * `status` - The HTTP status the probe came back with.
/// * `body` - The response body.
///
/// # Returns
///
/// [`ProbeVerdict::Controller`] only when the answer could not have come from
/// something else.
fn judge_probe(_status: u16, body: &str) -> ProbeVerdict {
    // Skeleton: today any page whose markup so much as mentions UniFi counts.
    if body.contains("window.UNIFI_")
        || body.contains("UniFi")
        || body.contains("ui-icon")
        || body.contains("/api/login")
    {
        ProbeVerdict::Controller
    } else {
        ProbeVerdict::NotController
    }
}

/// Validate a controller URL provided by the user
pub async fn validate_user_url(url: &str) -> Result<DiscoveredController> {
    let parsed = url::Url::parse(url).context("Invalid URL format")?;

    let host = parsed.host_str().context("URL must have a host")?;

    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });

    validate_controller(host, port).await
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    /// What a controller's integration API answers an unauthenticated `info`
    /// request with once a key is supplied.
    const CONTROLLER_INFO: &str = r#"{"applicationVersion":"9.0.108"}"#;

    /// A router's own status page, a documentation page, a search result --
    /// anything that merely says the word.
    const A_PAGE_THAT_MENTIONS_UNIFI: &str = r#"<!doctype html><html><head>
        <title>Best UniFi deals</title></head><body>
        <div class="ui-icon"></div><a href="/api/login">Sign in</a>
        <p>Our UniFi controller review</p></body></html>"#;

    /// The genuine article: the endpoint answers with the shape the API
    /// documents.
    #[test]
    fn a_controllers_info_response_is_a_controller() {
        assert_eq!(
            judge_probe(200, CONTROLLER_INFO),
            ProbeVerdict::Controller,
            "the integration API's own answer must be recognised"
        );
    }

    /// "You need a key for this" can only come from something that has the
    /// endpoint, which is itself the evidence.
    #[test]
    fn an_unauthenticated_rejection_is_a_controller() {
        for status in [401, 403] {
            assert_eq!(
                judge_probe(status, r#"{"statusCode":401,"message":"Unauthorized"}"#),
                ProbeVerdict::Controller,
                "a {status} from the integration API means the endpoint is there"
            );
        }
    }

    /// The setup wizard offers what discovery found, so a page that merely
    /// contains the word must not become a "found controller".
    #[test]
    fn a_page_that_merely_mentions_unifi_is_not_a_controller() {
        assert_eq!(
            judge_probe(200, A_PAGE_THAT_MENTIONS_UNIFI),
            ProbeVerdict::NotController,
            "markup mentioning UniFi is not evidence of a controller"
        );
    }

    /// No such endpoint is evidence *against*, however friendly the 404 body.
    #[test]
    fn a_missing_endpoint_is_not_a_controller() {
        assert_eq!(
            judge_probe(404, r#"{"statusCode":404,"message":"UniFi not found"}"#),
            ProbeVerdict::NotController,
            "a missing integration API means this is not a controller"
        );
    }

    /// Some other JSON API answering happily on the same port is still not a
    /// controller.
    #[test]
    fn unrelated_json_is_not_a_controller() {
        assert_eq!(
            judge_probe(200, r#"{"status":"ok","service":"printer"}"#),
            ProbeVerdict::NotController,
            "a 200 without the documented shape proves nothing"
        );
    }
}
