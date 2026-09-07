//! An HTTP server a test runs, which records what a client sent it.
//!
//! Two of the properties this crate must hold are properties of a request as
//! the *server* reads it. A client that follows a redirect hands the user's
//! API key to the second host, and a client with no bound waits forever for a
//! host that never answers. Neither is visible from the client side alone, so
//! a test needs a server of its own. This is that server.
//!
//! The server binds port zero, so the operating system picks the port. Two
//! copies of one test therefore never fight over an address.

use std::fmt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// The address a test server listens on.
const LISTEN_ADDRESS: &str = "127.0.0.1:0";

/// The bytes that end a request head.
const HEAD_END: &[u8] = b"\r\n\r\n";

/// How much of a request head a server reads before it gives up.
///
/// A request this crate makes carries a handful of headers. The bound exists
/// only so a client that never sends the end of the head cannot grow the
/// buffer without limit.
const MAX_HEAD_BYTES: usize = 65_536;

/// How much a server reads off a socket at a time.
const READ_CHUNK_BYTES: usize = 1_024;

/// What a [`TestServer`] writes back once it has read a request.
#[derive(Clone)]
enum Reply {
    /// These bytes, exactly as given.
    Bytes(String),
    /// Nothing at all. The server keeps the connection open, so a client
    /// waits for an answer rather than reads an end of file.
    Silence,
}

/// One request a [`TestServer`] read.
#[derive(Clone)]
pub struct ReceivedRequest {
    /// The first line, such as `GET /info HTTP/1.1`.
    request_line: String,
    /// Every header, with the name in lower case.
    headers: Vec<(String, String)>,
}

impl fmt::Debug for ReceivedRequest {
    /// Show the request the way a server log does: the request line, then
    /// every header on one line.
    ///
    /// The derived form would do as well, but a derive does not count as a
    /// read of a field, so `request_line` would be dead code to the compiler
    /// while a failed assertion prints it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.request_line)?;
        for (name, value) in &self.headers {
            write!(formatter, " | {name}: {value}")?;
        }
        Ok(())
    }
}

impl ReceivedRequest {
    /// Read a request head.
    ///
    /// # Arguments
    ///
    /// * `head` - The head of the request, without the empty line that ends it.
    ///
    /// # Returns
    ///
    /// The request line and every header it carried.
    fn parse(head: &str) -> Self {
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
            .collect();

        Self {
            request_line,
            headers,
        }
    }

    /// The value this request carried for `name`.
    ///
    /// # Arguments
    ///
    /// * `name` - The header name, in lower case. HTTP header names are not
    ///   case sensitive, and this server folds every name it reads to lower
    ///   case.
    ///
    /// # Returns
    ///
    /// The header value, or `None` when the request carried no such header.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An HTTP server for one test.
///
/// The server accepts every connection, records the request head, and writes
/// back the one canned response it was built with. It stops when the test
/// drops it.
pub struct TestServer {
    /// The origin a client reaches this server at.
    origin: String,
    /// Every request this server read.
    requests: Arc<Mutex<Vec<ReceivedRequest>>>,
    /// The task that accepts the connections.
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    /// Start a server that answers every request with `response`.
    ///
    /// # Arguments
    ///
    /// * `response` - The raw HTTP response, headers included.
    ///
    /// # Returns
    ///
    /// The running server.
    pub async fn replying(response: &str) -> Self {
        Self::start(Reply::Bytes(response.to_string())).await
    }

    /// Start a server that accepts a connection and never answers it.
    ///
    /// The connection stays open, so a client reads no end of file and no
    /// refusal. Only a bound of its own ends the request.
    ///
    /// # Returns
    ///
    /// The running server.
    pub async fn silent() -> Self {
        Self::start(Reply::Silence).await
    }

    /// Start a server that answers every request the one way `reply` says.
    ///
    /// # Arguments
    ///
    /// * `reply` - What to write back.
    ///
    /// # Returns
    ///
    /// The running server.
    ///
    /// # Panics
    ///
    /// Panics if the loopback address cannot be bound.
    async fn start(reply: Reply) -> Self {
        let listener = TcpListener::bind(LISTEN_ADDRESS)
            .await
            .expect("a test server must be able to bind a loopback port");
        let port = listener
            .local_addr()
            .expect("a bound listener has a local address")
            .port();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);

        let task = tokio::spawn(async move {
            // A silent server must not drop the connection: an end of file is
            // an answer of a kind, and a client ends the request on it.
            let mut held_open = Vec::new();

            while let Ok((mut socket, _)) = listener.accept().await {
                let Some(request) = read_request(&mut socket).await else {
                    continue;
                };
                recorded
                    .lock()
                    .expect("nothing panics while it holds this lock")
                    .push(request);

                match &reply {
                    Reply::Bytes(response) => {
                        let _ = socket.write_all(response.as_bytes()).await;
                        let _ = socket.flush().await;
                    }
                    Reply::Silence => held_open.push(socket),
                }
            }
        });

        Self {
            origin: format!("http://127.0.0.1:{port}"),
            requests,
            task,
        }
    }

    /// The origin a client reaches this server at, such as
    /// `http://127.0.0.1:52341`.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every request this server has read so far.
    ///
    /// # Returns
    ///
    /// The requests, in the order they arrived.
    ///
    /// # Panics
    ///
    /// Panics if the server task panicked while it held the record.
    pub fn requests(&self) -> Vec<ReceivedRequest> {
        self.requests
            .lock()
            .expect("nothing panics while it holds this lock")
            .clone()
    }
}

/// Read the head of one request off `socket`.
///
/// # Arguments
///
/// * `socket` - The accepted connection.
///
/// # Returns
///
/// The request, or `None` when the client sent no complete head.
async fn read_request(socket: &mut TcpStream) -> Option<ReceivedRequest> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; READ_CHUNK_BYTES];

    loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);

        if let Some(end) = head
            .windows(HEAD_END.len())
            .position(|window| window == HEAD_END)
        {
            return Some(ReceivedRequest::parse(&String::from_utf8_lossy(
                &head[..end],
            )));
        }

        if head.len() > MAX_HEAD_BYTES {
            return None;
        }
    }
}

/// The response that sends a client to `location`.
///
/// # Arguments
///
/// * `location` - The URL the client is sent to.
///
/// # Returns
///
/// A raw `302 Found` response.
pub fn redirect_to(location: &str) -> String {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
}

/// The response that carries `body` as its JSON document.
///
/// # Arguments
///
/// * `body` - The JSON document to answer with.
///
/// # Returns
///
/// A raw `200 OK` response that carries `body`.
pub fn json_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// The response that carries an empty JSON object.
///
/// # Returns
///
/// A raw `200 OK` response with a body every JSON reader in this crate
/// accepts.
pub fn empty_json() -> String {
    json_response("{}")
}
