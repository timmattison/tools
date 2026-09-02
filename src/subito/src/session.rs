//! The MQTT session of `subito`.
//!
//! The module connects, subscribes, prints every message that arrives, and
//! stops. It takes the connection options rather than builds them, so a test
//! points it at a local broker and the presigner and TLS stay out of the test.
//!
//! Two defects of the Go tool this one replaces live here. The Go tool printed
//! `Subscribed` before the broker answered, so a topic the policy denied looked
//! the same as a topic that works, and the tool then stayed silent forever.
//! The Go tool also ended with `select {}`, so an interrupt killed the process
//! with the MQTT session open. This module prints what the broker answered, it
//! stops when the broker refuses every topic, and it sends a DISCONNECT before
//! it gives control back.

use crate::payload::format_payload;
use rumqttc::{
    AsyncClient, Event, MqttOptions, Outgoing, Packet, Publish, QoS, SubAck, SubscribeReasonCode,
};
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;
use std::time::Duration;

/// The count of requests the channel holds above the count of the topics.
///
/// The channel takes one request for each topic, and one more for the
/// DISCONNECT of the clean shutdown. A channel that is full holds the sender
/// until the event loop takes a request, and the shutdown path sends the
/// DISCONNECT while nothing polls the event loop, so the one spare place is
/// what keeps that path from a deadlock.
const SPARE_CAPACITY: usize = 1;

/// The wait after the first failure of a connection.
const FIRST_WAIT: Duration = Duration::from_secs(1);

/// The longest wait between one attempt to connect and the next.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// The quality of service "at most once", as a number.
const QOS_AT_MOST_ONCE: u8 = 0;

/// The quality of service "at least once", as a number.
const QOS_AT_LEAST_ONCE: u8 = 1;

/// The quality of service "exactly once", as a number.
const QOS_EXACTLY_ONCE: u8 = 2;

/// A failure of the MQTT session.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The event loop did not accept a request.
    ///
    /// The client sends a subscription and a disconnection to the event loop
    /// through a channel. This failure says the event loop stopped, so no
    /// request reaches the broker.
    #[error("the MQTT event loop did not accept a request")]
    Request(#[source] rumqttc::ClientError),

    /// The connection to the broker failed.
    ///
    /// The box keeps the variant small. The connection error holds a whole
    /// MQTT packet in one of its variants, which makes it larger than every
    /// other failure of this enum together.
    #[error("the MQTT connection failed")]
    Connection(#[source] Box<rumqttc::ConnectionError>),

    /// The broker refused every subscription.
    ///
    /// A tool that subscribed to nothing prints nothing, and a user cannot see
    /// the difference between a policy that denies the topic and a topic that
    /// no publisher writes to. So this is a failure and not a quiet wait.
    #[error("the broker refused every subscription")]
    AllSubscriptionsRefused,

    /// The output did not take a line the session printed.
    #[error("the session could not print to its output")]
    Output(#[source] std::io::Error),

    /// The process could not wait for the interrupt signal.
    #[error("the process could not wait for the interrupt signal")]
    Signal(#[source] std::io::Error),
}

/// The wait between one attempt to run a session and the next.
///
/// The wait starts at `first`, doubles after each failure, and stops at
/// `longest`. A connection that subscribes takes the wait back to `first`.
///
/// The type is a parameter of [`run_forever_with`] because a test cannot wait
/// a second for each attempt. [`run_forever`] takes [`Backoff::default`],
/// which is the policy the tool ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    /// The wait after the first failure.
    first: Duration,

    /// The longest wait this policy ever asks for.
    longest: Duration,
}

impl Backoff {
    /// Builds a policy that starts at `first` and doubles up to `longest`.
    #[must_use]
    pub const fn new(first: Duration, longest: Duration) -> Self {
        Self { first, longest }
    }
}

impl Default for Backoff {
    /// Gives the policy the tool ships: one second, doubling, up to thirty.
    fn default() -> Self {
        Self::new(FIRST_WAIT, LONGEST_WAIT)
    }
}

/// Runs a session again after every failure a new connection can repair.
///
/// AWS IoT Core signs the WebSocket URL of a connection, and the signature
/// covers the handshake alone. A second attempt with the same URL therefore
/// presents a stale signature, so `connect` builds the options again for each
/// attempt, from credentials it reads again.
///
/// The Go tool this one replaces runs until the user stops it, because the
/// MQTT client under it reconnects on its own. This function keeps that.
///
/// # Errors
///
/// Gives [`SessionError::AllSubscriptionsRefused`] when the broker refuses
/// every topic, because a policy that denies every topic denies it again, and
/// [`SessionError::Signal`] when the process cannot wait for the interrupt.
/// Every other failure starts another attempt.
pub async fn run_forever<F, Fut>(
    connect: F,
    topics: &[String],
    qos: QoS,
    pretty_json: bool,
    output: &mut impl Write,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<(), SessionError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<MqttOptions, SessionError>>,
{
    run_forever_with(
        connect,
        topics,
        qos,
        pretty_json,
        output,
        shutdown,
        Backoff::default(),
    )
    .await
}

/// Runs a session again after every failure, and waits as `backoff` states.
///
/// `backoff` is a parameter, and not the policy the tool ships, so a test does
/// not wait a second for each attempt.
///
/// # Errors
///
/// Gives the failures of [`run_forever`].
pub async fn run_forever_with<F, Fut>(
    connect: F,
    topics: &[String],
    qos: QoS,
    pretty_json: bool,
    output: &mut impl Write,
    shutdown: impl Future<Output = std::io::Result<()>>,
    backoff: Backoff,
) -> Result<(), SessionError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<MqttOptions, SessionError>>,
{
    let mut shutdown = std::pin::pin!(shutdown);
    let mut waits = Waits::new(backoff);

    loop {
        let failure = match connect().await {
            Ok(options) => {
                let ended =
                    attempt(options, topics, qos, pretty_json, output, shutdown.as_mut()).await;

                // The connection worked far enough to pass the policy of the
                // broker, so the next failure starts from the first wait.
                if ended.granted {
                    waits.reset();
                }

                match ended.result {
                    Ok(()) => return Ok(()),
                    Err(failure) => failure,
                }
            }
            Err(failure) => failure,
        };

        let wait = waits.take();
        report_retry(&failure, wait, output)?;
        tokio::time::sleep(wait).await;
    }
}

/// The wait of the next attempt, as one [`Backoff`] states it.
struct Waits {
    /// The policy this state follows.
    policy: Backoff,

    /// The wait the next failure takes.
    next: Duration,
}

impl Waits {
    /// Starts a policy at its first wait.
    fn new(policy: Backoff) -> Self {
        Self {
            policy,
            next: policy.first,
        }
    }

    /// Gives the wait of this failure, and doubles the wait of the next one.
    fn take(&mut self) -> Duration {
        let wait = self.next;
        self.next = wait.saturating_mul(2).min(self.policy.longest);

        wait
    }

    /// Takes the wait back to the first one.
    fn reset(&mut self) {
        self.next = self.policy.first;
    }
}

/// Prints what failed and how long the tool waits before the next attempt.
///
/// # Errors
///
/// Gives [`SessionError::Output`] when the output refuses the line.
fn report_retry(
    failure: &SessionError,
    wait: Duration,
    output: &mut impl Write,
) -> Result<(), SessionError> {
    writeln!(output, "{}. Trying again in {wait:?}.", describe(failure))
        .map_err(SessionError::Output)
}

/// Gives the message of a failure, and of every failure under it.
///
/// The message of [`SessionError`] alone names the part that failed and not
/// the reason, because the reason belongs to the error under it. A user who
/// reads one line of a reconnect needs both.
fn describe(failure: &SessionError) -> String {
    let mut text = failure.to_string();
    let mut under = std::error::Error::source(failure);

    while let Some(error) = under {
        text.push_str(": ");
        text.push_str(&error.to_string());
        under = error.source();
    }

    text
}

/// Connects, subscribes, and prints every message until an interrupt arrives.
///
/// This is the entrance the binary takes. The interrupt is
/// [`tokio::signal::ctrl_c`], and it ends the run with a DISCONNECT, so the
/// broker sees the session close instead of a connection that stops answering.
///
/// # Errors
///
/// Gives the failures of [`run_until`].
pub async fn run(
    options: MqttOptions,
    topics: &[String],
    qos: QoS,
    pretty_json: bool,
    output: &mut impl Write,
) -> Result<(), SessionError> {
    run_until(
        options,
        topics,
        qos,
        pretty_json,
        output,
        tokio::signal::ctrl_c(),
    )
    .await
}

/// Connects, subscribes, and prints every message until `shutdown` answers.
///
/// `shutdown` is a parameter, and not a call to [`tokio::signal::ctrl_c`], so a
/// test ends the run through a channel. A signal sent to the test process would
/// end the whole test run.
///
/// The run stops for one of four reasons: `shutdown` answered, the broker
/// refused every subscription, the connection failed, or the output refused a
/// line.
///
/// # Errors
///
/// Gives [`SessionError::Request`] when the event loop stops before it takes a
/// request, [`SessionError::Connection`] when the connection to the broker
/// fails, [`SessionError::AllSubscriptionsRefused`] when the broker refuses
/// every topic, [`SessionError::Output`] when the output refuses a line, and
/// [`SessionError::Signal`] when the process cannot wait for the interrupt.
pub async fn run_until(
    options: MqttOptions,
    topics: &[String],
    qos: QoS,
    pretty_json: bool,
    output: &mut impl Write,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<(), SessionError> {
    attempt(options, topics, qos, pretty_json, output, shutdown)
        .await
        .result
}

/// What one attempt to run a session did.
struct Attempt {
    /// The end of the attempt.
    result: Result<(), SessionError>,

    /// Whether the broker accepted one subscription of this attempt.
    ///
    /// An attempt that got this far reached the broker and passed its policy,
    /// so the supervisor takes its wait back to the first one.
    granted: bool,
}

/// Runs one session, from the connection to the end.
///
/// [`run_until`] gives the answer of this function, and the supervisor reads
/// the whole answer, because a failure alone does not say whether the attempt
/// ever worked.
async fn attempt(
    mut options: MqttOptions,
    topics: &[String],
    qos: QoS,
    pretty_json: bool,
    output: &mut impl Write,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Attempt {
    // `rumqttc` rolls `last_pkid` back to zero at `max_inflight`, so a run
    // with more topics than that limit reuses a packet identifier before the
    // first SUBACK arrives, and the answer then names the wrong topic.
    raise_inflight_limit(&mut options, topics.len());

    let (client, mut eventloop) = AsyncClient::new(options, topics.len() + SPARE_CAPACITY);

    for topic in topics {
        if let Err(error) = client.subscribe(topic.clone(), qos).await {
            return Attempt {
                result: Err(SessionError::Request(error)),
                granted: false,
            };
        }
    }

    let mut subscriptions = Subscriptions::new(topics);

    let result = drive(
        &client,
        &mut eventloop,
        &mut subscriptions,
        pretty_json,
        output,
        shutdown,
    )
    .await;

    Attempt {
        result,
        granted: subscriptions.any_granted(),
    }
}

/// Gives `options` one packet identifier for each topic of the run.
///
/// `rumqttc` counts its packet identifiers up to `max_inflight` and then
/// starts again at one. A run with more topics than that limit therefore gives
/// one identifier to two topics, and the second SUBACK to arrive names the
/// wrong one. The identifiers of a subscription are free of the flow control
/// this limit exists for, because this tool sends no message of its own.
///
/// The limit never goes down. A caller that asked for a larger one keeps it.
fn raise_inflight_limit(options: &mut MqttOptions, topics: usize) {
    let needed = u16::try_from(topics).unwrap_or(u16::MAX);

    if options.inflight() < needed {
        options.set_inflight(needed);
    }
}

/// Drives the event loop until the run stops.
///
/// The shutdown and the event loop wait together, so an interrupt that arrives
/// while nothing is on the wire still ends the run. An interrupt then goes
/// through [`say_goodbye`], and every other end of the run gives its own
/// failure.
async fn drive(
    client: &AsyncClient,
    eventloop: &mut rumqttc::EventLoop,
    subscriptions: &mut Subscriptions,
    pretty_json: bool,
    output: &mut impl Write,
    shutdown: impl Future<Output = std::io::Result<()>>,
) -> Result<(), SessionError> {
    let mut shutdown = std::pin::pin!(shutdown);

    loop {
        let event = tokio::select! {
            signal = shutdown.as_mut() => {
                signal.map_err(SessionError::Signal)?;
                return say_goodbye(client, eventloop).await;
            }
            event = eventloop.poll() => {
                event.map_err(|error| SessionError::Connection(Box::new(error)))?
            }
        };

        match event {
            Event::Outgoing(Outgoing::Subscribe(pkid)) => subscriptions.record(pkid),
            Event::Incoming(Packet::SubAck(ack)) => {
                subscriptions.answer(&ack, output)?;

                if subscriptions.every_subscription_refused() {
                    return Err(SessionError::AllSubscriptionsRefused);
                }
            }
            Event::Incoming(Packet::Publish(publish)) => {
                print_message(&publish, pretty_json, output)?;
            }
            _ => (),
        }
    }
}

/// Closes the MQTT session and waits until the DISCONNECT is on the wire.
///
/// The Go tool this one replaces ended with `select {}`, so an interrupt
/// killed the process with the session open, and the broker learned of the end
/// only when the connection timed out. This function sends the DISCONNECT the
/// protocol states, and it waits, because a process that exits before the
/// packet leaves the machine sends nothing.
///
/// The event loop writes the packet and flushes it before it gives the
/// `Outgoing::Disconnect` event, so that event says the broker holds the
/// packet. A connection that ends first carries the same meaning: nothing more
/// can go out, and there is nothing left to wait for.
///
/// # Errors
///
/// Gives [`SessionError::Request`] when the event loop stops before it takes
/// the disconnection.
async fn say_goodbye(
    client: &AsyncClient,
    eventloop: &mut rumqttc::EventLoop,
) -> Result<(), SessionError> {
    client.disconnect().await.map_err(SessionError::Request)?;

    loop {
        match eventloop.poll().await {
            Ok(Event::Outgoing(Outgoing::Disconnect)) | Err(_) => return Ok(()),
            Ok(_) => (),
        }
    }
}

/// Prints one message: its topic, its payload, and a blank line.
///
/// The payload goes through [`format_payload`], so a payload that holds an
/// escape sequence arrives as a hex dump and does not change the terminal of
/// the user.
///
/// # Errors
///
/// Gives [`SessionError::Output`] when the output refuses a line.
fn print_message(
    publish: &Publish,
    pretty_json: bool,
    output: &mut impl Write,
) -> Result<(), SessionError> {
    let topic = &publish.topic;
    let message = format_payload(&publish.payload, pretty_json);

    writeln!(output, "Topic: {topic}").map_err(SessionError::Output)?;
    writeln!(output, "Message: {message}").map_err(SessionError::Output)?;
    writeln!(output).map_err(SessionError::Output)?;

    Ok(())
}

/// What the broker answered about one topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The broker accepted the subscription.
    Granted,

    /// The broker refused the subscription.
    Refused,
}

/// One topic of a run, and what the broker answered about it.
struct Topic {
    /// The topic filter, as the caller gave it.
    name: String,

    /// What the broker answered, or nothing while no answer arrived.
    answer: Option<Answer>,
}

/// The topics of one run, and the packet identifier each one carries.
///
/// MQTT names a subscription by a packet identifier, and the broker answers
/// with that identifier and no topic. A broker answers in any order, so the
/// order the answers arrive names nothing. This type holds the one pairing that
/// does: the identifier the client chose for each topic.
struct Subscriptions {
    /// The topics, in the order the caller gave them.
    topics: Vec<Topic>,

    /// The topic each packet identifier belongs to, as an index into `topics`.
    by_pkid: HashMap<u16, usize>,

    /// The index of the topic that takes the next packet identifier.
    ///
    /// The event loop takes one subscription at a time, in the order the
    /// client sent them, so the identifiers arrive in the order of the topics.
    next: usize,
}

impl Subscriptions {
    /// Takes the topics of one run, in the order the caller gave them.
    fn new(topics: &[String]) -> Self {
        Self {
            topics: topics
                .iter()
                .map(|name| Topic {
                    name: name.clone(),
                    answer: None,
                })
                .collect(),
            by_pkid: HashMap::new(),
            next: 0,
        }
    }

    /// Pairs one packet identifier with the topic it belongs to.
    fn record(&mut self, pkid: u16) {
        if self.next < self.topics.len() {
            self.by_pkid.insert(pkid, self.next);
            self.next += 1;
        }
    }

    /// Says whether the broker accepted one topic of the run.
    fn any_granted(&self) -> bool {
        self.topics
            .iter()
            .any(|topic| topic.answer == Some(Answer::Granted))
    }

    /// Says whether the broker refused every topic of the run.
    ///
    /// A run of no topics is not a run the broker refused, and the answer of a
    /// topic that no answer arrived for is not a refusal. So this gives true
    /// only after every topic has an answer and every answer is a refusal.
    fn every_subscription_refused(&self) -> bool {
        !self.topics.is_empty()
            && self
                .topics
                .iter()
                .all(|topic| topic.answer == Some(Answer::Refused))
    }

    /// Prints what the broker answered about one subscription.
    ///
    /// A grant prints the topic and the quality of service the broker gave,
    /// which is not always the one the client asked for. A refusal says so,
    /// and the run goes on, because the other topics of the run still work.
    ///
    /// An answer for an identifier this run never sent names no topic, so the
    /// session prints nothing for it.
    ///
    /// # Errors
    ///
    /// Gives [`SessionError::Output`] when the output refuses a line.
    fn answer(&mut self, ack: &SubAck, output: &mut impl Write) -> Result<(), SessionError> {
        let Some(topic) = self
            .by_pkid
            .get(&ack.pkid)
            .copied()
            .and_then(|index| self.topics.get_mut(index))
        else {
            return Ok(());
        };

        let name = &topic.name;

        for code in &ack.return_codes {
            match code {
                SubscribeReasonCode::Success(granted) => {
                    writeln!(output, "Subscribed: {name} (QoS {})", qos_number(*granted))
                        .map_err(SessionError::Output)?;
                    topic.answer = Some(Answer::Granted);
                }
                SubscribeReasonCode::Failure => {
                    writeln!(output, "Subscription refused: {name}")
                        .map_err(SessionError::Output)?;
                    topic.answer = Some(Answer::Refused);
                }
            }
        }

        Ok(())
    }
}

/// Gives the number MQTT states for one quality of service.
///
/// The match is complete, so a new quality of service in a later version of
/// the MQTT client breaks the build instead of printing the wrong number.
fn qos_number(qos: QoS) -> u8 {
    match qos {
        QoS::AtMostOnce => QOS_AT_MOST_ONCE,
        QoS::AtLeastOnce => QOS_AT_LEAST_ONCE,
        QoS::ExactlyOnce => QOS_EXACTLY_ONCE,
    }
}
