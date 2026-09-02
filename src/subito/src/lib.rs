//! `subito` — subscribe to AWS IoT Core topics over a signed WebSocket.
//!
//! The crate holds the parts of the tool that a test can drive without a
//! network and without an AWS account. [`cli`] states the command line,
//! [`payload`] turns the bytes of one MQTT message into the text the tool
//! prints, [`presign`] builds the signed WebSocket URL that AWS IoT Core
//! accepts for an MQTT connection, and [`endpoint`] asks AWS for the name of
//! the data endpoint of the account, because a user knows the region and does
//! not know that name. [`session`] then connects with those parts, subscribes
//! to each topic, prints every message that arrives, sends a DISCONNECT when
//! an interrupt ends the run, and builds the connection again after a failure
//! a new connection repairs.
//!
//! [`payload::format_payload`] is the part that keeps a terminal safe. An MQTT
//! payload is a byte string of any content, and a byte string that holds an
//! escape sequence changes the terminal that prints it. The function gives
//! text only for a payload that is text, and a hex dump for every other
//! payload.

#![cfg_attr(not(test), warn(clippy::unwrap_used))]
#![cfg_attr(not(test), warn(clippy::expect_used))]

pub mod cli;
pub mod endpoint;
pub mod payload;
pub mod presign;
pub mod session;
