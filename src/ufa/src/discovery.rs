use crate::{client::integration_api_base, models::ApplicationInfo};
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::collections::HashSet;
use std::future::Future;
use std::net::IpAddr;
use std::time::Duration;
use tokio::time::timeout;
use url::Url;

/// The integration API endpoint every controller answers, relative to the
/// base URL that [`integration_api_base`] gives.
const INFO_ENDPOINT: &str = "info";

/// How long a single host gets to answer a probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct DiscoveredController {
    pub ip: String,
    pub port: u16,
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
/// Every URL that `ufa` builds from a host and a port comes from here. The
/// host goes through [`url::Host`], which writes an IPv6 address in the
/// brackets a URL needs. The port stays in the URL even when it is 443:
/// `Url::set_port` removes the default port of the scheme, and a list of
/// controllers shows 443 and 8443 side by side.
///
/// # Arguments
///
/// * `host` - An IPv4 address, an IPv6 address with or without brackets, or
///   a domain name.
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
    let host = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => url::Host::Ipv4(address),
        Ok(IpAddr::V6(address)) => url::Host::Ipv6(address),
        Err(_) => {
            url::Host::parse(host).with_context(|| format!("No URL can hold the host {host:?}"))?
        }
    };

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

    Ok(without_duplicates(controllers))
}

/// Keep the first controller at each IP address and port, and remove each
/// later one.
///
/// mDNS and the probe of the common addresses can find one controller two
/// times, and `unifi` and `unifi.local` can resolve to one address. The port
/// is part of the rule: a console and a Network application on one host
/// answer on 443 and on 8443, and the user must be able to pick either of
/// them.
///
/// # Arguments
///
/// * `controllers` - The controllers in the order that discovery found them.
///
/// # Returns
///
/// The controllers, with each repeated pair of IP address and port removed.
fn without_duplicates(controllers: Vec<DiscoveredController>) -> Vec<DiscoveredController> {
    let mut seen = HashSet::new();
    controllers
        .into_iter()
        .filter(|controller| seen.insert((controller.ip.clone(), controller.port)))
        .collect()
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
        if validate_controller(&controller.ip, controller.port)
            .await
            .is_ok()
        {
            controller.is_verified = true;
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
/// Discovery calls this function, and it has no URL that a user typed. A URL
/// that the user typed goes through [`validate_user_url`], which keeps the
/// name.
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
/// Returns an error if no URL can hold `host`, if the host cannot be reached,
/// or if it does not answer the integration API.
pub async fn validate_controller(host: &str, port: u16) -> Result<DiscoveredController> {
    let origin = Url::parse(&origin_url(host, port)?).context("Invalid controller URL")?;

    match probe(probe_url(&origin)?).await? {
        ProbeVerdict::NotController => {
            anyhow::bail!("{host}:{port} does not answer the UniFi integration API")
        }
        ProbeVerdict::Controller => Ok(DiscoveredController {
            ip: resolve_address(host, port).await,
            port,
            is_verified: true,
        }),
    }
}

/// The URL of the integration API's `info` endpoint of the controller at
/// `controller`.
///
/// The client finds the integration API with the same rule, so a controller
/// behind a reverse proxy is probed under its path prefix.
///
/// # Arguments
///
/// * `controller` - The URL of the controller, with or without a path
///   prefix.
///
/// # Returns
///
/// The URL to probe.
///
/// # Errors
///
/// Returns an error if the endpoint cannot be joined onto the base URL.
fn probe_url(controller: &Url) -> Result<Url> {
    integration_api_base(controller)
        .join(INFO_ENDPOINT)
        .context("Failed to construct the probe URL")
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
/// Returns an error if the host cannot be reached, or if it answers with a
/// redirect. The client of each later command refuses a redirect, so a
/// controller that the probe reaches only through one is of no use. The error
/// names the status and the `Location` of the redirect. When the redirect
/// keeps the path of the probe and goes to another controller, the error also
/// names the URL of the controller to type.
async fn probe(url: Url) -> Result<ProbeVerdict> {
    // Controllers ship a self-signed certificate out of the box, so discovery
    // -- which runs before any trust decision has been made -- cannot insist
    // on a valid one.
    #[expect(
        clippy::disallowed_methods,
        reason = "a probe carries no API key, so it needs no header map of build_client, and it states the redirect policy of build_client itself"
    )]
    let client = crate::http::with_ufa_transport(reqwest::Client::builder())
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .build()?;

    let response = client.get(url.clone()).send().await?;
    let status = response.status();
    if status.is_redirection() {
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok());
        let advice = location
            .and_then(|location| controller_behind_redirect(&url, location))
            .map(|controller| format!(". Type {controller} instead"))
            .unwrap_or_default();
        let location = location.unwrap_or("no Location");
        anyhow::bail!(
            "{url} answered {status} and sends the client to {location}. \
             ufa does not follow a redirect, so give the URL of the controller itself{advice}"
        );
    }
    let body = response.text().await.unwrap_or_default();

    Ok(judge_probe(status.as_u16(), &body))
}

/// Name the URL of the controller that a refused redirect points at.
///
/// The probe asks the `info` endpoint, so a redirect that keeps the path, as a
/// redirect from `http` to `https` does, names that endpoint. The client
/// appends the integration path to each URL it gets, so the user must type the
/// URL without it.
///
/// # Arguments
///
/// * `probed` - The URL of the probe that met the redirect.
/// * `location` - The `Location` of the redirect, absolute or relative.
///
/// # Returns
///
/// The URL of the controller to type, with no probe path, query, fragment or
/// trailing slash. `None` if the `Location` does not end in the path that the
/// probe adds, or if the probe of the named controller is `probed` itself.
/// Such a redirect, for example one that only sets a cookie or adds an empty
/// segment, names the URL that just failed.
fn controller_behind_redirect(probed: &Url, location: &str) -> Option<String> {
    let target = probed.join(location).ok()?;
    let mut root = target.clone();
    root.set_path("/");
    let probe_path = probe_url(&root).ok()?.path().to_string();
    let prefix = target.path().strip_suffix(probe_path.as_str())?;
    let mut controller = target.clone();
    controller.set_path(prefix);
    controller.set_query(None);
    controller.set_fragment(None);
    if probe_url(&controller).ok()?[..url::Position::AfterPath]
        == probed[..url::Position::AfterPath]
    {
        return None;
    }
    Some(controller.as_str().trim_end_matches('/').to_string())
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
/// The probe goes to the URL the user typed, path prefix included. The URL to
/// save is that URL, unchanged. A controller with a valid certificate for its
/// name passes verification only at that name, so an address in its place
/// fails on every run.
///
/// # Arguments
///
/// * `url` - The URL the user typed.
///
/// # Returns
///
/// The URL to save, which is `url` unchanged.
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
/// The URL to save, which is `url` unchanged.
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
    let typed = Url::parse(url).context("Invalid URL format")?;
    anyhow::ensure!(typed.has_host(), "URL must have a host");

    match probe(probe_url(&typed)?).await? {
        ProbeVerdict::NotController => {
            anyhow::bail!("{url} does not answer the UniFi integration API")
        }
        ProbeVerdict::Controller => Ok(url.to_string()),
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
    use crate::test_server::{json_response, redirect_to, ReceivedRequest, TestServer};
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

    /// The client refuses each redirect, so a URL that reaches the controller
    /// only through a redirect fails each later command. The probe refuses
    /// the redirect too, and its error names the status and the place the
    /// redirect goes, so the wizard shows the user where it goes.
    #[tokio::test]
    async fn a_typed_url_that_redirects_fails_and_names_where_it_goes() {
        let controller = TestServer::replying(&json_response(CONTROLLER_INFO)).await;
        let location = format!("{}/proxy/network/integration/v1/info", controller.origin());
        let redirecting = TestServer::replying(&redirect_to(&location)).await;

        let outcome = validate_user_url(redirecting.origin()).await;

        assert!(
            controller.requests().is_empty(),
            "the probe followed the redirect, which the client refuses: {:?}",
            controller.requests()
        );
        let error = format!(
            "{:#}",
            outcome.expect_err("a URL that answers with a redirect must not validate")
        );
        assert!(
            error.contains("302"),
            "the error must name the status of the redirect, got: {error}"
        );
        assert!(
            error.contains(&location),
            "the error must name where the redirect goes, got: {error}"
        );
    }

    /// A redirect that keeps the path of the probe, as a redirect from `http`
    /// to `https` does, points at the `info` endpoint of the controller, not
    /// at the URL to type. The client appends the integration path to each URL
    /// it gets, so the error names the URL of the controller without that
    /// path.
    #[tokio::test]
    async fn a_redirect_that_keeps_the_probe_path_names_the_controller_url_to_type() {
        let controller = TestServer::replying(&json_response(CONTROLLER_INFO)).await;
        let location = format!("{}/proxy/network/integration/v1/info", controller.origin());
        let redirecting = TestServer::replying(&redirect_to(&location)).await;
        let error = format!(
            "{:#}",
            validate_user_url(redirecting.origin())
                .await
                .expect_err("a URL that answers with a redirect must not validate")
        );
        assert!(
            error.contains(&format!("Type {} instead", controller.origin())),
            "the error must name the URL of the controller to type, got: {error}"
        );

        let controller2 = TestServer::replying(&json_response(CONTROLLER_INFO)).await;
        let location2 = format!(
            "{}/unifi/proxy/network/integration/v1/info",
            controller2.origin()
        );
        let redirecting2 = TestServer::replying(&redirect_to(&location2)).await;
        let error2 = format!(
            "{:#}",
            validate_user_url(redirecting2.origin())
                .await
                .expect_err("a URL that answers with a redirect must not validate")
        );
        assert!(
            error2.contains(&format!("Type {}/unifi instead", controller2.origin())),
            "the error must keep the path prefix, got: {error2}"
        );

        let controller3 = TestServer::replying(&json_response(CONTROLLER_INFO)).await;
        let location3 = format!("{}/login", controller3.origin());
        let redirecting3 = TestServer::replying(&redirect_to(&location3)).await;
        let error3 = format!(
            "{:#}",
            validate_user_url(redirecting3.origin())
                .await
                .expect_err("a URL that answers with a redirect must not validate")
        );
        assert!(
            !error3.contains("Type "),
            "a redirect that does not keep the probe path names no URL to type, got: {error3}"
        );
    }

    /// A redirect back to the URL of the probe, as a redirect that only sets a
    /// cookie does, names no URL to type. The controller it points at is the
    /// URL that just failed, and that URL fails the same way on the next try.
    /// A change of the query or the fragment alone points at the same
    /// controller. So does an empty segment before the probe path, because
    /// the client removes the trailing slash of the controller URL.
    #[tokio::test]
    async fn a_redirect_to_the_probe_url_itself_names_no_url_to_type() {
        let cases = [
            ("", "/proxy/network/integration/v1/info"),
            ("", "/proxy/network/integration/v1/info?session=1"),
            ("", "/proxy/network/integration/v1/info#top"),
            ("/unifi", "/unifi/proxy/network/integration/v1/info"),
            ("/unifi", "/unifi//proxy/network/integration/v1/info"),
        ];
        for (prefix, location) in cases {
            let redirecting = TestServer::replying(&redirect_to(location)).await;
            let typed = format!("{}{prefix}", redirecting.origin());
            let error = format!(
                "{:#}",
                validate_user_url(&typed)
                    .await
                    .expect_err("a URL that answers with a redirect must not validate")
            );
            assert!(
                error.contains(location),
                "the error must name where the redirect goes, got: {error}"
            );
            assert!(
                !error.contains("Type "),
                "a redirect to the probe URL itself names no URL to type, got: {error}"
            );
        }
    }

    /// An absolute `Location` with an empty segment before the probe path
    /// names the root of the host that the probe asked. The probe of that
    /// root is the URL that just failed.
    #[test]
    fn an_absolute_redirect_to_the_probed_root_names_no_url_to_type() {
        let probed = Url::parse("http://host/proxy/network/integration/v1/info")
            .expect("the probe URL must parse");

        assert_eq!(
            controller_behind_redirect(&probed, "http://host//proxy/network/integration/v1/info"),
            None,
            "a redirect to the probed controller names no URL to type"
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

#[cfg(test)]
mod duplicate_tests {
    use super::*;

    /// A controller at `ip` and `port`, as a probe of discovery gives it.
    fn found(ip: &str, port: u16, is_verified: bool) -> DiscoveredController {
        DiscoveredController {
            ip: ip.to_string(),
            port,
            is_verified,
        }
    }

    /// The address and the port of each controller, in order.
    fn origins(controllers: &[DiscoveredController]) -> Vec<(String, u16)> {
        controllers
            .iter()
            .map(|controller| (controller.ip.clone(), controller.port))
            .collect()
    }

    /// A console and a Network application on one host answer on 443 and on
    /// 8443. The two are different controllers, and the user must be able to
    /// pick either of them.
    #[test]
    fn two_ports_of_one_address_are_two_controllers() {
        let kept = without_duplicates(vec![
            found("192.168.1.1", 443, true),
            found("192.168.1.1", 8443, true),
        ]);

        assert_eq!(
            origins(&kept),
            vec![
                ("192.168.1.1".to_string(), 443),
                ("192.168.1.1".to_string(), 8443)
            ],
            "a second port of one address is a second controller"
        );
    }

    /// mDNS and the probe of the common addresses can find one controller two
    /// times. The first one stays. The second copy is not verified only so
    /// that the test can tell the two copies apart.
    #[test]
    fn one_address_and_port_found_two_times_is_one_controller() {
        let kept = without_duplicates(vec![
            found("192.168.1.1", 443, true),
            found("192.168.1.1", 443, false),
        ]);

        assert_eq!(kept.len(), 1, "one address and port is one controller");
        assert!(
            kept.first()
                .is_some_and(|controller| controller.is_verified),
            "the first controller stays"
        );
    }
}
