//! `subito` — subscribe to AWS IoT Core topics over a signed WebSocket.
//!
//! The crate holds the parts of the tool that a test can drive without a
//! network and without an AWS account. [`cli`] states the command line,
//! [`payload`] turns the bytes of one MQTT message into the text the tool
//! prints, [`presign`] builds the signed WebSocket URL that AWS IoT Core
//! accepts for an MQTT connection, and [`endpoint`] asks AWS for the name of
//! the data endpoint of the account, because a user knows the region and does
//! not know that name. [`session`] then connects with those parts, subscribes
//! to each topic, prints every message that arrives, stops at once when an
//! interrupt arrives, and builds the connection again after a failure a new
//! connection repairs. An interrupt that finds a session open sends a
//! DISCONNECT, and an interrupt that arrives between two sessions has no
//! connection to close. [`session`] writes to two sinks: the messages take the
//! topic and the payload of each message, and the reports take what the broker
//! answered and what the supervisor does after a failure. The binary gives
//! standard output for the messages and standard error for the reports.
//!
//! [`payload`] is the part that keeps a terminal safe. An MQTT payload is a
//! byte string of any content, and an MQTT topic name carries every character
//! other than the null character, so a publisher chooses both. A byte string
//! that holds an escape sequence changes the terminal that prints it, and a
//! byte string that holds a character that changes the direction of the text
//! makes the terminal print a line in an order the bytes do not have.
//! [`payload::format_payload`] gives text only for a payload that is text, and
//! [`payload::format_topic`] gives text only for a topic that is text. Every
//! other payload and every other topic gives a hex dump.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

pub mod cli;
pub mod endpoint;
pub mod payload;
pub mod presign;
pub mod session;
