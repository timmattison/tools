use crate::{client::INTEGRATION_API_PATH, models::ApplicationInfo};
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::HashSet;
use std::future::Future;
use std::time::Duration;
use tokio::time::timeout;
use url::Url;

/// The integration API endpoint every controller answers, relative to
/// [`INTEGRATION_API_PATH`].
const INFO_ENDPOINT: &str = "info";

/// How long a single host gets to answer a probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct DiscoveredController {
    pub ip: String,
    pub port: u16,
    pub name: Option<String>,
    pub is_verified: bool,
}

impl DiscoveredController {
    /// The URL of this controller.
    ///
    /// # Returns
    ///
    /// The origin URL, as [`origin_url`] builds it.
    ///
    /// # Errors
    ///
    /// Returns an error if no URL can hold the address.
    pub fn url(&self) -> Result<String> {
        origin_url(&self.ip, self.port)
    }
}

/// The origin URL of a controller at `host` and `port`.
///
/// # Arguments
///
/// * `host` - An IP address or a domain name.
/// * `port` - The HTTPS port.
///
/// # Returns
///
/// The URL `https://<host>:<port>`.
///
/// # Errors
///
/// Returns an error if no URL can hold `host`.
pub fn origin_url(host: &str, port: u16) -> Result<String> {
    Ok(format!("https://{host}:{port}"))
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

/// Discover controllers via mDNS.
///
/// # Returns
///
/// Every controller that answered the browse and then passed a probe.
///
/// # Errors
///
/// Returns an error if the mDNS daemon cannot be started or cannot browse.
/// The stop request goes out on every path out, including those errors.
async fn discover_via_mdns() -> Result<Vec<DiscoveredController>> {
    let mdns = StopOnDrop::new(ServiceDaemon::new()?);
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

/// Validate that a given host:port is a UniFi controller.
///
/// The host is asked for the integration API's `info` endpoint -- the one
/// thing only a controller has -- rather than for its front page, because a
/// front page proves nothing: any host that merely *mentions* UniFi would
/// otherwise be offered to the user as a controller to configure.
///
/// # Arguments
///
/// * `host` - Hostname or IP address to probe.
/// * `port` - HTTPS port to probe.
///
/// # Returns
///
/// The controller, with `host` resolved to an address where it was a name.
///
/// # Errors
///
/// Returns an error if the host cannot be reached or does not answer the
/// integration API.
pub async fn validate_controller(host: &str, port: u16) -> Result<DiscoveredController> {
    let url = Url::parse(&format!(
        "https://{host}:{port}{INTEGRATION_API_PATH}{INFO_ENDPOINT}"
    ))?;

    match probe(url).await? {
        ProbeVerdict::NotController => {
            anyhow::bail!("{host}:{port} does not answer the UniFi integration API")
        }
        ProbeVerdict::Controller => Ok(DiscoveredController {
            ip: resolve_address(host, port).await,
            port,
            name: None,
            is_verified: true,
        }),
    }
}

/// Ask `url` for the integration API's `info` endpoint, and judge the answer.
///
/// # Arguments
///
/// * `url` - The URL of the `info` endpoint to ask.
///
/// # Returns
///
/// What the answer says about the host.
///
/// # Errors
///
/// Returns an error if the host cannot be reached.
async fn probe(url: Url) -> Result<ProbeVerdict> {
    // Controllers ship a self-signed certificate out of the box, so discovery
    // -- which runs before any trust decision has been made -- cannot insist
    // on a valid one.
    #[expect(
        clippy::disallowed_methods,
        reason = "a probe carries no API key, so it needs no client that guards one, and it follows a redirect on purpose: the host behind one is still a controller"
    )]
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(PROBE_TIMEOUT)
        .build()?;

    let response = client.get(url).send().await?;
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();

    Ok(judge_probe(status, &body))
}

/// Resolve `host` to an address, falling back to the name itself.
///
/// Name resolution blocks, and discovery probes eight addresses at once on
/// the async runtime, so this uses tokio's resolver rather than
/// [`std::net::ToSocketAddrs`] -- the blocking one would park a runtime
/// worker per probe for however long the resolver takes to give up.
async fn resolve_address(host: &str, port: u16) -> String {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return host.to_string();
    }

    tokio::net::lookup_host((host, port))
        .await
        .ok()
        .and_then(|mut addrs| addrs.next())
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| host.to_string())
}

/// A background service that keeps running until it is told to stop.
trait Stoppable {
    /// Ask the service to stop.
    ///
    /// The request goes out and the call returns. The service thread ends
    /// after it reads the request, which is later than this call returns.
    fn stop(&self);
}

impl Stoppable for ServiceDaemon {
    fn stop(&self) {
        // `shutdown` sends the daemon an exit command and hands back a
        // receiver for the status of it. This call drops that receiver, so
        // the request goes out and nothing waits for the daemon to act on it.
        // A send failure means the daemon has already stopped.
        let _ = self.shutdown();
    }
}

/// Asks what it holds to stop when it goes out of scope -- including when an
/// early `?` is what takes the scope away.
///
/// `ServiceDaemon` runs a background thread for the life of the process
/// unless it is asked to stop, and discovery creates one every time it runs,
/// so a path that forgets the request leaves a thread behind for good.
struct StopOnDrop<T: Stoppable>(T);

impl<T: Stoppable> StopOnDrop<T> {
    /// Take ownership of `service` so it cannot outlive this scope.
    fn new(service: T) -> Self {
        Self(service)
    }
}

impl<T: Stoppable> Drop for StopOnDrop<T> {
    fn drop(&mut self) {
        self.0.stop();
    }
}

impl<T: Stoppable> std::ops::Deref for StopOnDrop<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
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
fn judge_probe(status: u16, body: &str) -> ProbeVerdict {
    /// The endpoint is there but wants a key. Nothing else serves a
    /// challenge at this path, so the challenge is the evidence.
    const NEEDS_CREDENTIALS: std::ops::RangeInclusive<u16> = 401..=403;
    /// Answers the request outright.
    const SUCCEEDED: std::ops::RangeInclusive<u16> = 200..=299;

    if NEEDS_CREDENTIALS.contains(&status) {
        return ProbeVerdict::Controller;
    }

    // A success has to carry the shape the API documents: some other service
    // answering 200 on the same port is not a controller, and neither is a
    // captive portal that rewrote the request into an HTML page.
    if SUCCEEDED.contains(&status) && serde_json::from_str::<ApplicationInfo>(body).is_ok() {
        return ProbeVerdict::Controller;
    }

    ProbeVerdict::NotController
}

/// Validate a controller URL the user typed.
///
/// # Arguments
///
/// * `url` - The URL the user typed.
///
/// # Returns
///
/// The URL to save.
///
/// # Errors
///
/// Returns an error if the URL does not parse, or if no controller answers
/// the integration API there.
pub async fn validate_user_url(url: &str) -> Result<String> {
    validate_typed_url(url, probe).await
}

/// Validate a controller URL the user typed, with `probe` to ask the host.
///
/// # Arguments
///
/// * `url` - The URL the user typed.
/// * `probe` - Asks the URL it gets for the `info` endpoint and judges the
///   answer. [`validate_user_url`] gives [`probe`]. A test gives a stand-in,
///   so it needs no network.
///
/// # Returns
///
/// The URL to save.
///
/// # Errors
///
/// Returns an error if the URL does not parse, if the probe fails, or if the
/// probe finds no controller.
async fn validate_typed_url<P, F>(url: &str, probe: P) -> Result<String>
where
    P: FnOnce(Url) -> F,
    F: Future<Output = Result<ProbeVerdict>>,
{
    let parsed = Url::parse(url).context("Invalid URL format")?;

    let host = parsed.host_str().context("URL must have a host")?;

    let port = parsed
        .port()
        .unwrap_or(if parsed.scheme() == "https" { 443 } else { 80 });

    let probe_url = Url::parse(&format!(
        "https://{host}:{port}{INTEGRATION_API_PATH}{INFO_ENDPOINT}"
    ))?;

    match probe(probe_url).await? {
        ProbeVerdict::NotController => {
            anyhow::bail!("{host}:{port} does not answer the UniFi integration API")
        }
        ProbeVerdict::Controller => DiscoveredController {
            ip: resolve_address(host, port).await,
            port,
            name: None,
            is_verified: true,
        }
        .url(),
    }
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

#[cfg(test)]
mod typed_url_tests {
    use super::*;
    use crate::test_server::{json_response, ReceivedRequest, TestServer};
    use std::cell::RefCell;

    /// What a controller's integration API answers an `info` request with.
    const CONTROLLER_INFO: &str = r#"{"applicationVersion":"9.0.108"}"#;

    /// Validate `url` with a stand-in probe that finds a controller.
    ///
    /// # Returns
    ///
    /// The outcome, and the URL the probe got.
    async fn validate_at_a_controller(url: &str) -> (Result<String>, Option<Url>) {
        let asked = RefCell::new(None);
        let outcome = validate_typed_url(url, |probe_url| {
            *asked.borrow_mut() = Some(probe_url);
            async { Ok(ProbeVerdict::Controller) }
        })
        .await;
        (outcome, asked.into_inner())
    }

    /// A controller with a valid certificate for its name passes
    /// verification only at that name. An address in its place fails
    /// verification on every run, and the error then tells the user to use
    /// `--insecure`.
    #[tokio::test]
    async fn a_typed_hostname_that_validates_is_saved_as_typed() {
        const TYPED: &str = "https://localhost";

        let (outcome, _) = validate_at_a_controller(TYPED).await;

        assert_eq!(
            outcome.expect("a controller answered the probe").as_str(),
            TYPED,
            "the URL to save must be the URL the user typed"
        );
    }

    /// A controller behind a reverse proxy answers under a path prefix. The
    /// probe asks for the integration API under that prefix, as the client
    /// does, and the saved URL keeps the prefix.
    #[tokio::test]
    async fn a_typed_path_prefix_is_probed_and_saved() {
        const TYPED: &str = "https://localhost/unifi";

        let (outcome, asked) = validate_at_a_controller(TYPED).await;

        assert_eq!(
            asked.expect("the probe must run").as_str(),
            "https://localhost/unifi/proxy/network/integration/v1/info",
            "the probe must ask for the integration API under the prefix"
        );
        assert_eq!(
            outcome.expect("a controller answered the probe").as_str(),
            TYPED,
            "the URL to save must keep the prefix"
        );
    }

    /// A URL that already names the integration API gets `info` and no
    /// second copy of the integration path, as the client does.
    #[tokio::test]
    async fn a_typed_integration_url_is_probed_where_it_stands() {
        let (_, asked) =
            validate_at_a_controller("https://localhost/proxy/network/integration/v1").await;

        assert_eq!(
            asked.expect("the probe must run").as_str(),
            "https://localhost/proxy/network/integration/v1/info"
        );
    }

    /// The real probe goes to the URL the user typed: the same scheme, the
    /// same port and the same path prefix.
    #[tokio::test]
    async fn the_probe_goes_to_the_url_the_user_typed() {
        let controller = TestServer::replying(&json_response(CONTROLLER_INFO)).await;
        let typed = format!("{}/unifi", controller.origin());

        let outcome = validate_user_url(&typed).await;

        let received = controller.requests();
        assert_eq!(
            received
                .iter()
                .map(ReceivedRequest::request_line)
                .collect::<Vec<_>>(),
            ["GET /unifi/proxy/network/integration/v1/info HTTP/1.1"],
            "the probe must reach the integration API under the typed URL"
        );
        assert_eq!(
            outcome.expect("the test server answers as a controller"),
            typed,
            "the URL to save must be the URL the user typed"
        );
    }
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    /// A discovered controller at the given address, on port 443.
    fn controller_at(address: &str) -> DiscoveredController {
        DiscoveredController {
            ip: address.to_string(),
            port: 443,
            name: None,
            is_verified: true,
        }
    }

    /// An IPv6 address in a URL needs brackets. Without them, the colons of
    /// the address and the colon of the port run together, and no URL parser
    /// accepts the result.
    #[test]
    fn an_ipv6_address_gets_the_brackets_a_url_needs() {
        let url = controller_at("2001:db8::1")
            .url()
            .expect("an IPv6 address fits in a URL");

        assert_eq!(url, "https://[2001:db8::1]:443");
        let parsed = Url::parse(&url).expect("the URL must parse");
        assert_eq!(
            parsed.host(),
            Some(url::Host::Ipv6(
                "2001:db8::1".parse().expect("a valid address")
            )),
            "the host must be the IPv6 address"
        );
    }

    /// An address that already has its brackets keeps one pair of them.
    #[test]
    fn a_bracketed_ipv6_address_keeps_one_pair_of_brackets() {
        assert_eq!(
            origin_url("[2001:db8::1]", 443).expect("a bracketed IPv6 address fits in a URL"),
            "https://[2001:db8::1]:443"
        );
    }

    /// An IPv4 address and a name keep the explicit port, as before.
    #[test]
    fn an_ipv4_address_and_a_name_keep_the_explicit_port() {
        assert_eq!(
            controller_at("192.168.1.1")
                .url()
                .expect("an IPv4 address fits in a URL"),
            "https://192.168.1.1:443"
        );
        assert_eq!(
            origin_url("unifi.local", 8443).expect("a name fits in a URL"),
            "https://unifi.local:8443"
        );
    }

    /// A host that no URL can hold gives an error, not a URL that every
    /// later request fails to parse.
    #[test]
    fn a_host_no_url_can_hold_is_refused() {
        let outcome = origin_url("port forward", 8443);

        assert!(
            outcome.is_err(),
            "a host with a space in it fits in no URL, got {outcome:?}"
        );
    }
}

#[cfg(test)]
mod stop_on_drop_tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A stand-in for the mDNS daemon. It records the stop request instead
    /// of a bind of a multicast socket, so the test never touches the
    /// network.
    struct FakeService {
        stopped: Rc<Cell<bool>>,
    }

    impl Stoppable for FakeService {
        fn stop(&self) {
            self.stopped.set(true);
        }
    }

    /// Build a service alongside the flag that watches it.
    fn watched_service() -> (StopOnDrop<FakeService>, Rc<Cell<bool>>) {
        let stopped = Rc::new(Cell::new(false));
        let service = FakeService {
            stopped: Rc::clone(&stopped),
        };
        (StopOnDrop::new(service), stopped)
    }

    /// The ordinary path: the scope ends and the stop request goes out with
    /// it.
    #[test]
    fn leaving_the_scope_stops_the_service() {
        let (guard, stopped) = watched_service();

        assert!(!stopped.get(), "the service runs while the guard is alive");
        drop(guard);

        assert!(
            stopped.get(),
            "the guard must ask the service to stop as it goes out of scope"
        );
    }

    /// The path that gets forgotten: an early `?` unwinds the scope before
    /// any hand-written cleanup would have run.
    #[test]
    fn an_early_question_mark_still_stops_the_service() {
        let stopped = Rc::new(Cell::new(false));

        let outcome: Result<()> = (|| {
            let _guard = StopOnDrop::new(FakeService {
                stopped: Rc::clone(&stopped),
            });
            anyhow::bail!("browsing failed");
        })();

        assert!(outcome.is_err(), "the scope must have ended early");
        assert!(
            stopped.get(),
            "an error path must ask the service to stop too, or the thread stays"
        );
    }
}
