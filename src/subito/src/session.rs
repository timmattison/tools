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
    AsyncClient, Event, EventLoop, MqttOptions, Outgoing, Packet, Publish, QoS, SubAck,
    SubscribeReasonCode,
};
use std::collections::HashMap;
use std::future::Future;
use std::io::Write;

/// The count of requests the channel holds above the count of the topics.
///
/// The channel takes one request for each topic, and one more for the
/// DISCONNECT of the clean shutdown. A channel that is full holds the sender
/// until the event loop takes a request, and the shutdown path sends the
/// DISCONNECT while nothing polls the event loop, so the one spare place is
/// what keeps that path from a deadlock.
const SPARE_CAPACITY: usize = 1;

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
    unimplemented!("the session does not run yet")
}
