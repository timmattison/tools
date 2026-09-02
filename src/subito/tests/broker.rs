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
use rumqttc::mqttbytes::v4::{ConnAck, ConnectReturnCode, Packet, SubAck, SubscribeReasonCode};
use rumqttc::{MqttOptions, QoS, Transport};
use std::time::{Duration, SystemTime};
use subito::session::{run_until, SessionError};
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
    #[allow(
        clippy::result_large_err,
        reason = "the `Callback` trait of `tungstenite` states the answer of a handshake callback, and its `Err` variant is a whole HTTP response. This callback never builds one: it adds the subprotocol header and gives the response back"
    )]
    async fn accept(listener: TcpListener) -> Self {
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
}

/// A shutdown that never answers, for a test that ends the run another way.
fn never() -> impl std::future::Future<Output = std::io::Result<()>> {
    std::future::pending()
}

#[tokio::test]
async fn a_suback_names_the_topic_of_its_own_packet_identifier() {
    let (listener, port) = listening().await;
    let topics = vec!["sensors/one".to_string(), "sensors/two".to_string()];
    let (mut output, mut printed) = recording();

    let session = run_until(
        options_for(port),
        &topics,
        QoS::AtMostOnce,
        false,
        &mut output,
        never(),
    );

    let script = async {
        let mut broker = Broker::accept(listener).await;
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
            printed.lines(2).await,
            [
                "Subscribed: sensors/two (QoS 0)",
                "Subscribed: sensors/one (QoS 0)"
            ]
        );

        // The connection ends, so the run gives control back to the test.
        drop(broker);
    };

    let (result, ()) = tokio::join!(session, script);

    assert!(
        matches!(result, Err(SessionError::Connection(_))),
        "a connection that ends is a failure of the connection: {result:?}"
    );
}
