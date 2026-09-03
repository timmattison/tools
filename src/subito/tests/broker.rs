//! The session of `subito`, driven against a broker inside the test process.
//!
//! The broker binds a port the operating system chooses, speaks the WebSocket
//! handshake that `rumqttc` demands, and then reads and writes MQTT 3.1.1
//! packets with the packet reader and writer of `rumqttc` itself. So the test
//! reaches no network, holds no AWS credential, and needs no fixed port.
//!
//! The transport is a plain WebSocket. The presigner and TLS carry their own
//! tests, and neither belongs in a test of the session.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "every unwrap and expect in this file is an assertion about the harness the test just built: the listener it bound, the socket it accepted, the packets it wrote, and the lines the session printed back through a channel this file owns. A failure of one of them is a broken harness and not behavior under test, so it must stop the test where it happens and name what was missing. The failures of the session itself are never unwrapped: each test reads the `Result` of `run_until` and asserts on the variant. src/lib.rs raises both lints under cfg(not(test)) for that reason"
)]

use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use rumqttc::mqttbytes::v4::{
    ConnAck, ConnectReturnCode, Packet, Publish, SubAck, SubscribeReasonCode,
};
use rumqttc::{MqttOptions, QoS, Transport};
use std::time::{Duration, SystemTime};
use subito::session::{run_forever_with, run_until, Backoff, SessionError};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

/// The largest MQTT packet the broker reads or writes.
///
/// This is the limit `rumqttc` puts on its own client, so a packet the broker
/// accepts is a packet the client accepts.
const MAX_PACKET_SIZE: usize = 10 * 1024;

/// The longest time one step of a test waits.
///
/// Every wait of this file carries this limit, so a test that waits for
/// something that never arrives fails with a message that names it. A test
/// that hangs blocks every commit of this repository.
const STEP_TIMEOUT: Duration = Duration::from_secs(20);

/// The longest time a whole test waits for the session and the broker.
///
/// This limit is longer than [`STEP_TIMEOUT`], so a step that waits for
/// something that never arrives fails with its own message and this one stays
/// as the last guard against a test that never ends.
const RUN_TIMEOUT: Duration = Duration::from_secs(30);

/// The header that names the subprotocol of a WebSocket.
const SUBPROTOCOL_HEADER: &str = "Sec-WebSocket-Protocol";

/// The subprotocol AWS IoT Core and `rumqttc` demand.
///
/// `validate_response_headers` of `rumqttc` refuses a handshake answer that
/// does not carry this value, so the broker must send it.
const MQTT_SUBPROTOCOL: &str = "mqtt";

/// The path of the MQTT WebSocket.
const MQTT_PATH: &str = "mqtt";

/// The address the broker binds. Port zero asks the operating system to choose.
const ANY_LOCAL_PORT: &str = "127.0.0.1:0";

/// Gives a client identifier that no other run of this test shares.
///
/// Two copies of this test can run at one time. The identifier holds the
/// process and the moment, so neither copy takes the name of the other.
fn client_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock of this machine is before 1970")
        .as_nanos();

    format!("subito-test-{}-{nanos}", std::process::id())
}

/// Binds a listener on a port the operating system chooses.
async fn listening() -> (TcpListener, u16) {
    let listener = TcpListener::bind(ANY_LOCAL_PORT)
        .await
        .expect("the test could not bind a local port");
    let port = listener
        .local_addr()
        .expect("the listener of the test names no address")
        .port();

    (listener, port)
}

/// Builds the connection options that reach the broker of this test.
fn options_for(port: u16) -> MqttOptions {
    let mut options = MqttOptions::new(
        client_id(),
        format!("ws://127.0.0.1:{port}/{MQTT_PATH}"),
        port,
    );
    options.set_transport(Transport::ws());
    options
}

/// The output the session writes to.
///
/// Each write goes into a channel, so the test reads what the session printed
/// while the session is still running. A test that could only read the output
/// after the run would have to guess when to end the run, and a guess is a
/// race.
struct Recorder {
    sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
}

impl std::io::Write for Recorder {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.sender.send(buffer.to_vec()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the test dropped the reader of the output",
            )
        })?;

        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The text the session printed, as the session prints it.
struct Printed {
    receiver: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    text: String,
}

impl Printed {
    /// Waits until the session printed `count` whole lines, and gives them.
    ///
    /// The text stays, so a later call waits for more lines and gives every
    /// line from the first one.
    async fn lines(&mut self, count: usize) -> Vec<String> {
        while self.text.matches('\n').count() < count {
            let so_far = self.text.clone();

            let chunk = match tokio::time::timeout(STEP_TIMEOUT, self.receiver.recv()).await {
                Err(_) => panic!("the session did not print {count} lines. It printed: {so_far:?}"),
                Ok(None) => panic!(
                    "the session stopped printing before it printed {count} lines. It printed: {so_far:?}"
                ),
                Ok(Some(chunk)) => chunk,
            };

            self.text.push_str(
                &String::from_utf8(chunk).expect("the session printed bytes that are not UTF-8"),
            );
        }

        self.text.lines().take(count).map(str::to_string).collect()
    }

    /// Gives every byte this sink took up to this moment.
    ///
    /// The call takes what arrived already and waits for nothing, so a test
    /// that asserts one sink took nothing must first wait for a line on the
    /// other sink. The session writes the lines of one event before it reads
    /// the next event, so a line that arrived on the other sink says this sink
    /// took every byte the events up to that line give it. A call that waited
    /// for a line that never arrives could only end in the timeout of a step,
    /// which is no assertion at all.
    fn so_far(&mut self) -> String {
        while let Ok(chunk) = self.receiver.try_recv() {
            self.text.push_str(
                &String::from_utf8(chunk).expect("the session printed bytes that are not UTF-8"),
            );
        }

        self.text.clone()
    }
}

/// Builds an output for the session and the reader of that output.
fn recording() -> (Recorder, Printed) {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();

    (
        Recorder { sender },
        Printed {
            receiver,
            text: String::new(),
        },
    )
}

/// One subscription the broker read, as the SUBSCRIBE packet names it.
struct Subscription {
    /// The packet identifier the client chose.
    pkid: u16,
    /// The topic filter the client asked for.
    topic: String,
}

/// An MQTT broker that speaks WebSocket, inside the test process.
struct Broker {
    socket: WebSocketStream<TcpStream>,
    /// The bytes that arrived and do not yet make a whole packet.
    ///
    /// A WebSocket carries the MQTT byte stream inside binary messages, and one
    /// message holds any part of that stream: two packets, or half of one. So
    /// the broker keeps what arrived and reads a packet out of it only when the
    /// packet is whole.
    buffer: BytesMut,
}

impl Broker {
    /// Accepts one connection and completes the WebSocket handshake.
    ///
    /// The listener stays with the caller, so a test that watches a session
    /// reconnect accepts a second connection on the same port.
    #[allow(
        clippy::result_large_err,
        reason = "the `Callback` trait of `tungstenite` states the answer of a handshake callback, and its `Err` variant is a whole HTTP response. This callback never builds one: it adds the subprotocol header and gives the response back"
    )]
    async fn accept(listener: &TcpListener) -> Self {
        let (stream, _) = tokio::time::timeout(STEP_TIMEOUT, listener.accept())
            .await
            .expect("the session never opened a connection to the broker")
            .expect("the broker could not accept the connection of the session");

        let socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |_: &HandshakeRequest,
             mut response: HandshakeResponse|
             -> Result<HandshakeResponse, ErrorResponse> {
                // `rumqttc` refuses an answer that names no subprotocol.
                response.headers_mut().insert(
                    SUBPROTOCOL_HEADER,
                    HeaderValue::from_static(MQTT_SUBPROTOCOL),
                );
                Ok(response)
            },
        )
        .await
        .expect("the WebSocket handshake of the session failed");

        Self {
            socket,
            buffer: BytesMut::new(),
        }
    }

    /// Reads the next whole MQTT packet the client sent.
    async fn read_packet(&mut self) -> Packet {
        loop {
            match Packet::read(&mut self.buffer, MAX_PACKET_SIZE) {
                Ok(packet) => return packet,
                Err(rumqttc::mqttbytes::Error::InsufficientBytes(_)) => (),
                Err(error) => panic!("the client sent bytes that are not an MQTT packet: {error}"),
            }

            let message = tokio::time::timeout(STEP_TIMEOUT, self.socket.next())
                .await
                .expect("the client sent no more bytes, and the packet is not whole")
                .expect("the client closed the connection before it sent a whole packet")
                .expect("the WebSocket of the broker failed");

            match message {
                Message::Binary(bytes) => self.buffer.extend_from_slice(&bytes),
                Message::Ping(_) | Message::Pong(_) => (),
                other => panic!("the client sent a message that carries no MQTT bytes: {other:?}"),
            }
        }
    }

    /// Writes one MQTT packet as one binary WebSocket message.
    async fn write_packet(&mut self, packet: &Packet) {
        let mut bytes = BytesMut::new();
        packet
            .write(&mut bytes, MAX_PACKET_SIZE)
            .expect("the broker built a packet it cannot write");

        self.socket
            .send(Message::Binary(bytes.freeze()))
            .await
            .expect("the broker could not send a packet");
    }

    /// Reads the CONNECT of the client and accepts the connection.
    async fn accept_connection(&mut self) {
        match self.read_packet().await {
            Packet::Connect(_) => (),
            other => {
                panic!("the first packet of an MQTT session is a CONNECT, and this is {other:?}")
            }
        }

        self.write_packet(&Packet::ConnAck(ConnAck::new(
            ConnectReturnCode::Success,
            false,
        )))
        .await;
    }

    /// Reads one SUBSCRIBE and gives its packet identifier and its topic.
    async fn read_subscribe(&mut self) -> Subscription {
        match self.read_packet().await {
            Packet::Subscribe(subscribe) => {
                let filter = subscribe
                    .filters
                    .first()
                    .expect("a SUBSCRIBE names at least one topic filter");

                Subscription {
                    pkid: subscribe.pkid,
                    topic: filter.path.clone(),
                }
            }
            other => panic!("the client sent {other:?} where a SUBSCRIBE belongs"),
        }
    }

    /// Answers one subscription with a grant of `qos`.
    async fn grant(&mut self, pkid: u16, qos: QoS) {
        self.write_packet(&Packet::SubAck(SubAck::new(
            pkid,
            vec![SubscribeReasonCode::Success(qos)],
        )))
        .await;
    }

    /// Answers one subscription with a refusal.
    async fn refuse(&mut self, pkid: u16) {
        self.write_packet(&Packet::SubAck(SubAck::new(
            pkid,
            vec![SubscribeReasonCode::Failure],
        )))
        .await;
    }

    /// Sends one message to the client.
    async fn publish(&mut self, topic: &str, payload: &[u8]) {
        self.write_packet(&Packet::Publish(Publish::new(
            topic,
            QoS::AtMostOnce,
            payload.to_vec(),
        )))
        .await;
    }

    /// Reads the DISCONNECT of a clean shutdown.
    async fn read_disconnect(&mut self) {
        match self.read_packet().await {
            Packet::Disconnect => (),
            other => panic!("the client sent {other:?} where a DISCONNECT belongs"),
        }
    }

    /// Waits until the client closes the connection.
    ///
    /// The broker holds its end open, so the connection ends only when the run
    /// of the session ends. A client that sends another packet first fails the
    /// test, because a run that stopped sends nothing.
    async fn read_nothing_more(&mut self) {
        loop {
            let message = tokio::time::timeout(STEP_TIMEOUT, self.socket.next())
                .await
                .expect("the client held the connection open, and the run had to stop");

            match message {
                // The client closed the connection, in one way or the other.
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => (),
                Some(Ok(other)) => panic!("the client sent {other:?} after the run had to stop"),
            }
        }
    }
}

/// A shutdown that never answers, for a test that ends the run another way.
fn never() -> impl std::future::Future<Output = std::io::Result<()>> {
    std::future::pending()
}

/// Runs a session and the script of its broker together, and gives the answer
/// of the session.
///
/// Neither of the two finishes without the other, so the limit covers both. A
/// test that waits forever blocks every commit of this repository.
async fn together(
    session: impl std::future::Future<Output = Result<(), SessionError>>,
    script: impl std::future::Future<Output = ()>,
) -> Result<(), SessionError> {
    let (result, ()) = tokio::time::timeout(RUN_TIMEOUT, async { tokio::join!(session, script) })
        .await
        .expect("the session and the script of the broker did not both finish");

    result
}

#[tokio::test]
async fn a_suback_names_the_topic_of_its_own_packet_identifier() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/one".to_string(), "sensors/two".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let first = broker.read_subscribe().await;
        let second = broker.read_subscribe().await;
        assert_eq!(first.topic, "sensors/one");
        assert_eq!(second.topic, "sensors/two");

        // The broker answers the second subscription first. A tool that pairs
        // an answer with a topic by the order the answers arrive names the
        // wrong topic here. A tool that reads the packet identifier does not.
        broker.grant(second.pkid, QoS::AtMostOnce).await;
        broker.grant(first.pkid, QoS::AtMostOnce).await;

        assert_eq!(
            reported.lines(2).await,
            [
                "Subscribed: sensors/two (QoS 0)",
                "Subscribed: sensors/one (QoS 0)"
            ]
        );

        // The connection ends, so the run gives control back to the test.
        drop(broker);
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}

#[tokio::test]
async fn a_refused_subscription_says_so_and_the_run_goes_on() {
    let (listener, port) = listening().await;
    let topics = vec!["allowed/#".to_string(), "denied/#".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtLeastOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let allowed = broker.read_subscribe().await;
        let denied = broker.read_subscribe().await;

        broker.grant(allowed.pkid, QoS::AtLeastOnce).await;
        broker.refuse(denied.pkid).await;

        // The Go tool this one replaces printed `Subscribed` before the broker
        // answered, so a topic the policy denies looked the same as a topic
        // that works.
        assert_eq!(
            reported.lines(2).await,
            [
                "Subscribed: allowed/# (QoS 1)",
                "Subscription refused: denied/#"
            ]
        );

        drop(broker);
    };

    let result = together(session, script).await;

    // One topic of the two works, so the run goes on until the connection
    // ends. A run that stopped at the refusal gives another failure here.
    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a refusal of one topic of two does not end the run: {result:?}"
    );
}

#[tokio::test]
async fn a_run_whose_every_subscription_is_refused_stops() {
    let (listener, port) = listening().await;
    let topics = vec!["denied/one".to_string(), "denied/two".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let first = broker.read_subscribe().await;
        let second = broker.read_subscribe().await;

        broker.refuse(first.pkid).await;
        broker.refuse(second.pkid).await;

        assert_eq!(
            reported.lines(2).await,
            [
                "Subscription refused: denied/one",
                "Subscription refused: denied/two"
            ]
        );

        // The broker holds the connection open. A session that subscribed to
        // nothing must stop on its own, because it can never print a message.
        broker.read_nothing_more().await;
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::AllSubscriptionsRefused)),
        "a run that subscribed to nothing stops and says so: {result:?}"
    );
}

/// A payload of sixteen bytes that no terminal can print without damage.
///
/// The bytes hold `\x1b[31m`, which is the escape sequence that paints a
/// terminal red. A tool that writes such a payload to a terminal changes the
/// terminal of the user, which is the corruption the hex dump stops.
const UNPRINTABLE_PAYLOAD: &[u8] = b"binary\x00\x1b[31m\xff\xfe\xfd\xfc";

#[tokio::test]
async fn a_publish_prints_its_topic_and_its_payload() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, mut printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let filter = broker.read_subscribe().await;
        broker.grant(filter.pkid, QoS::AtMostOnce).await;

        assert_eq!(reported.lines(1).await, ["Subscribed: sensors/# (QoS 0)"]);

        broker
            .publish("sensors/kitchen", br#"{"temperature":21}"#)
            .await;
        broker.publish("sensors/hallway", UNPRINTABLE_PAYLOAD).await;

        assert_eq!(
            printed.lines(6).await,
            [
                // The topic of the message, and not the filter of the
                // subscription.
                "Topic: sensors/kitchen",
                r#"Message: {"temperature":21}"#,
                "",
                "Topic: sensors/hallway",
                // The payload printer is on this path, so a payload that no
                // terminal can print arrives as a hex dump.
                "Message: 00000000  62 69 6e 61 72 79 00 1b  5b 33 31 6d ff fe fd fc  |binary..[31m....|",
                "",
            ]
        );

        drop(broker);
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}

/// The topic of the one message the test of the two sinks reads.
const SPLIT_TOPIC: &str = "sensors/kitchen";

/// The payload of the one message the test of the two sinks reads.
const SPLIT_PAYLOAD: &[u8] = b"21";

#[tokio::test]
async fn a_report_and_a_message_go_to_two_sinks() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, mut printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let filter = broker.read_subscribe().await;
        broker.grant(filter.pkid, QoS::AtMostOnce).await;

        // The answer of the broker is a report, so a user who reads the
        // messages of a run through a pipe never sees this line.
        assert_eq!(reported.lines(1).await, ["Subscribed: sensors/# (QoS 0)"]);

        broker.publish(SPLIT_TOPIC, SPLIT_PAYLOAD).await;

        assert_eq!(
            printed.lines(3).await,
            ["Topic: sensors/kitchen", "Message: 21", ""]
        );

        // The session wrote the report before it read the message, and the
        // message is here already, so neither sink waits for a byte that is
        // still on the way.
        assert_eq!(
            reported.so_far(),
            "Subscribed: sensors/# (QoS 0)\n",
            "the reports hold the answer of the broker and no message"
        );
        assert_eq!(
            printed.so_far(),
            "Topic: sensors/kitchen\nMessage: 21\n\n",
            "the messages hold the message and no report"
        );

        drop(broker);
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}

/// A topic of nine bytes that no terminal can print without damage.
///
/// The last byte is the escape byte, which starts a terminal escape sequence.
/// MQTT forbids the null character in a topic name and forbids no other
/// control character, so a publisher can send this topic, and a run that
/// subscribes with a wildcard reads it.
const UNPRINTABLE_TOPIC: &str = "sensors/\u{1b}";

#[tokio::test]
async fn a_publish_prints_a_topic_that_no_terminal_can_print_as_a_hex_dump() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, mut printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let filter = broker.read_subscribe().await;
        broker.grant(filter.pkid, QoS::AtMostOnce).await;

        assert_eq!(reported.lines(1).await, ["Subscribed: sensors/# (QoS 0)"]);

        broker.publish(UNPRINTABLE_TOPIC, b"hello").await;

        assert_eq!(
            printed.lines(3).await,
            [
                // The topic printer is on this path, so a topic that no
                // terminal can print arrives as a hex dump, and the escape
                // byte of the publisher reaches no terminal.
                "Topic: 00000000  73 65 6e 73 6f 72 73 2f  1b                       |sensors/.|",
                "Message: hello",
                "",
            ]
        );

        drop(broker);
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}

#[tokio::test]
async fn the_interrupt_sends_a_disconnect_and_ends_the_run() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        async move {
            stopped.await.ok();
            Ok(())
        },
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let filter = broker.read_subscribe().await;
        broker.grant(filter.pkid, QoS::AtMostOnce).await;

        assert_eq!(reported.lines(1).await, ["Subscribed: sensors/# (QoS 0)"]);

        // A real signal would end the whole test run, so the interrupt of this
        // test arrives through the channel that `run_until` waits on.
        stop.send(())
            .expect("the session stopped before the interrupt arrived");

        // The Go tool this one replaces ended with `select {}`, so an
        // interrupt killed the process with the MQTT session open. This tool
        // says goodbye first.
        broker.read_disconnect().await;
    };

    let result = together(session, script).await;

    assert!(
        result.is_ok(),
        "a clean shutdown is the end of a run and not a failure: {result:?}"
    );
}

/// A count of topics above the in-flight limit `rumqttc` takes by default.
///
/// `rumqttc` rolls its packet identifier back to zero at the in-flight limit,
/// which is 100 unless a caller raises it. A run of more topics than that
/// therefore reuses an identifier before the first SUBACK arrives.
const MANY_TOPICS: usize = 120;

#[tokio::test]
async fn every_topic_of_a_list_longer_than_the_inflight_limit_keeps_its_own_line() {
    let (listener, port) = listening().await;
    let topics: Vec<String> = (0..MANY_TOPICS).map(|n| format!("sensors/{n}")).collect();
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let mut asked = Vec::with_capacity(MANY_TOPICS);
        for _ in 0..MANY_TOPICS {
            asked.push(broker.read_subscribe().await);
        }

        // The broker answers from the last subscription to the first, so an
        // identifier that two topics share names the wrong topic on one of the
        // two answers.
        for subscription in asked.iter().rev() {
            broker.grant(subscription.pkid, QoS::AtMostOnce).await;
        }

        let printed = reported.lines(MANY_TOPICS).await;
        let expected: Vec<String> = topics
            .iter()
            .rev()
            .map(|topic| format!("Subscribed: {topic} (QoS 0)"))
            .collect();

        let wrong: Vec<String> = expected
            .iter()
            .zip(&printed)
            .enumerate()
            .filter(|(_, (want, got))| want != got)
            .map(|(place, (want, got))| format!("line {place}: wanted {want:?}, printed {got:?}"))
            .collect();

        assert!(
            wrong.is_empty(),
            "{} of {MANY_TOPICS} lines name the wrong topic: {wrong:#?}",
            wrong.len()
        );

        drop(broker);
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}

/// The wait a test asks the supervisor for between two attempts.
///
/// The policy the tool ships starts at one second, which is a second this test
/// would spend waiting. The wait is a parameter of `run_forever_with` for that
/// reason.
const TEST_WAIT: Duration = Duration::from_millis(50);

/// The time a test watches a port for a connection that must not arrive.
///
/// The wait is far above [`TEST_WAIT`], so a supervisor that tries again
/// arrives inside it.
const NO_RETRY_WAIT: Duration = Duration::from_millis(500);

/// Gives a backoff that always waits [`TEST_WAIT`].
fn quick_backoff() -> Backoff {
    Backoff::new(TEST_WAIT, TEST_WAIT)
}

/// A wait so long that only a supervisor that watches the interrupt ends.
const SLOW_WAIT: Duration = Duration::from_secs(30);

/// The time a test gives a supervisor to answer an interrupt.
///
/// The time is far under [`SLOW_WAIT`], so a supervisor that answers the
/// interrupt only when its wait ends fails this test instead of passing it
/// thirty seconds later.
const PROMPT: Duration = Duration::from_secs(2);

/// Accepts one connection and closes it at once.
///
/// The client then sees the connection fail, which is what makes the
/// supervisor wait before it tries again.
async fn drop_one_connection(listener: &TcpListener) {
    let (stream, _) = tokio::time::timeout(STEP_TIMEOUT, listener.accept())
        .await
        .expect("the supervisor never opened a connection")
        .expect("the test could not accept the connection of the supervisor");

    drop(stream);
}

/// Fails when a second connection arrives within [`NO_RETRY_WAIT`].
async fn no_second_connection(listener: &TcpListener) {
    assert!(
        tokio::time::timeout(NO_RETRY_WAIT, listener.accept())
            .await
            .is_err(),
        "the supervisor opened a second connection, and this failure must never be tried again"
    );
}

/// Counts the attempts a supervisor made to build connection options.
#[derive(Clone)]
struct Attempts(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Attempts {
    fn new() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)))
    }

    fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn add_one(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_connection_that_drops_comes_back_and_subscribes_again() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let attempts = Attempts::new();

    let connect = {
        let attempts = attempts.clone();

        move || {
            let attempts = attempts.clone();

            async move {
                attempts.add_one();

                // Each attempt builds the options again. An AWS IoT URL holds
                // the signature of one handshake, so a second attempt with the
                // options of the first presents a stale signature.
                Ok(options_for(port))
            }
        }
    };

    let session = run_forever_with(
        connect,
        &topics,
        QoS::AtLeastOnce,
        false,
        &mut messages,
        &mut reports,
        async move {
            stopped.await.ok();
            Ok(())
        },
        quick_backoff(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;
        let first = broker.read_subscribe().await;
        broker.grant(first.pkid, QoS::AtMostOnce).await;

        assert_eq!(reported.lines(1).await, ["Subscribed: sensors/# (QoS 0)"]);

        // The network drops the connection.
        drop(broker);

        let told = reported.lines(2).await;
        assert!(
            told[1].ends_with(&format!("Trying again in {TEST_WAIT:?}.")),
            "the supervisor says what failed and how long it waits: {:?}",
            told[1]
        );

        // The session comes back on the same port, and it subscribes again,
        // because a new MQTT session carries no subscription of the old one.
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;
        let second = broker.read_subscribe().await;
        assert_eq!(second.topic, "sensors/#");
        broker.grant(second.pkid, QoS::AtLeastOnce).await;

        assert_eq!(
            reported.lines(3).await[2],
            "Subscribed: sensors/# (QoS 1)",
            "the second connection subscribes again"
        );
        assert_eq!(attempts.count(), 2, "each attempt builds its own options");

        stop.send(())
            .expect("the supervisor stopped before the interrupt arrived");
        broker.read_disconnect().await;
    };

    let result = together(session, script).await;

    assert!(
        result.is_ok(),
        "a clean shutdown is the end of a run and not a failure: {result:?}"
    );
}

#[tokio::test]
async fn a_policy_that_denies_every_topic_is_never_tried_again() {
    let (listener, port) = listening().await;
    let topics = vec!["denied/one".to_string(), "denied/two".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();
    let attempts = Attempts::new();

    let connect = {
        let attempts = attempts.clone();

        move || {
            let attempts = attempts.clone();

            async move {
                attempts.add_one();
                Ok(options_for(port))
            }
        }
    };

    let session = run_forever_with(
        connect,
        &topics,
        QoS::AtMostOnce,
        false,
        &mut messages,
        &mut reports,
        never(),
        quick_backoff(),
    );

    let script = async {
        let mut broker = Broker::accept(&listener).await;
        broker.accept_connection().await;

        let first = broker.read_subscribe().await;
        let second = broker.read_subscribe().await;
        broker.refuse(first.pkid).await;
        broker.refuse(second.pkid).await;

        assert_eq!(
            reported.lines(2).await,
            [
                "Subscription refused: denied/one",
                "Subscription refused: denied/two"
            ]
        );

        broker.read_nothing_more().await;

        // A policy that denies every topic denies it again, so another attempt
        // only reads the same answer.
        no_second_connection(&listener).await;
    };

    let result = together(session, script).await;

    assert!(
        matches!(result, Err(SessionError::AllSubscriptionsRefused)),
        "a run that subscribed to nothing stops and says so: {result:?}"
    );
    assert_eq!(attempts.count(), 1, "the supervisor connected once");
}

#[tokio::test]
async fn an_interrupt_ends_the_wait_between_two_attempts() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/#".to_string()];
    let (mut messages, _printed) = recording();
    let (mut reports, mut reported) = recording();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let (done, ended) = tokio::sync::oneshot::channel::<()>();

    let session = async {
        let result = run_forever_with(
            move || async move { Ok(options_for(port)) },
            &topics,
            QoS::AtMostOnce,
            false,
            &mut messages,
            &mut reports,
            async move {
                stopped.await.ok();
                Ok(())
            },
            Backoff::new(SLOW_WAIT, SLOW_WAIT),
        )
        .await;

        done.send(()).ok();

        result
    };

    let script = async {
        // The first attempt fails, so the supervisor starts its wait.
        drop_one_connection(&listener).await;

        let told = reported.lines(1).await;
        assert!(
            told[0].ends_with(&format!("Trying again in {SLOW_WAIT:?}.")),
            "the supervisor says how long it waits: {:?}",
            told[0]
        );

        // A user who presses Ctrl-C during that wait waits for nothing.
        stop.send(())
            .expect("the supervisor stopped before the interrupt arrived");

        tokio::time::timeout(PROMPT, ended)
            .await
            .expect("the supervisor held its wait and did not answer the interrupt")
            .expect("the supervisor ended without telling the test");
    };

    let result = together(session, script).await;

    assert!(
        result.is_ok(),
        "an interrupt ends the run and is not a failure: {result:?}"
    );
}
