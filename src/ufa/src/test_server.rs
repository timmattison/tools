//! An HTTP server a test runs, which records what a client sent it.
//!
//! Two of the properties this crate must hold are properties of a request as
//! the *server* reads it. A client that follows a redirect hands the user's
//! API key to the second host, and a client with no bound waits forever for a
//! host that never answers. Neither is visible from the client side alone, so
//! a test needs a server of its own. This is that server.
//!
//! The server also keeps the body of each request. A command that changes the
//! state of a controller says what to do in that body, and a wrong action in
//! it stays hidden until it runs against real hardware.
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

/// How much of a request body a server reads before it gives up.
///
/// A request this crate makes carries a small JSON document. The bound exists
/// only so a client that states a very large `Content-Length` cannot grow the
/// buffer without limit.
const MAX_BODY_BYTES: usize = 1_048_576;

/// How much a server reads off a socket at a time.
const READ_CHUNK_BYTES: usize = 1_024;

/// The line ending of an HTTP head.
const LINE_END: &str = "\r\n";

/// The header line that tells a client that the server closes the connection
/// after this answer.
const CONNECTION_CLOSE: &str = "Connection: close";

/// Where a controller serves the integration API, as the UniFi documentation
/// gives it.
///
/// A test writes the expected path from this text, not from the constant of
/// the client. An expected path built from the constant of the client passes
/// whatever that constant says.
pub const INTEGRATION_API: &str = "/proxy/network/integration/v1";

/// What a [`TestServer`] writes back once it has read a request.
#[derive(Clone)]
enum Reply {
    /// These bytes, exactly as given.
    Bytes(String),
    /// The response of the first route whose fragment is in the request
    /// line, or the fallback when no route matches.
    Routes {
        /// Pairs of a request-line fragment and the raw response for it.
        routes: Vec<(String, String)>,
        /// The raw response for a request that no route matches.
        fallback: String,
    },
    /// Nothing at all. The server keeps the connection open, so a client
    /// waits for an answer rather than reads an end of file.
    Silence,
    /// Nothing at all. The server closes the connection at once, so a client
    /// reads an end of file where the answer belongs.
    HangUp,
}

/// One request a [`TestServer`] read.
#[derive(Clone)]
pub struct ReceivedRequest {
    /// The first line, such as `GET /info HTTP/1.1`.
    request_line: String,
    /// Every header, with the name in lower case.
    headers: Vec<(String, String)>,
    /// The body, as text. It is empty when the request carried none.
    body: String,
}

impl fmt::Debug for ReceivedRequest {
    /// Show the request the way a server log does: the request line, every
    /// header, and then the body, all on one line.
    ///
    /// A derived form would print the field names and the vector brackets as
    /// well, which is harder to read in the message of a failed assertion
    /// than the shape the request arrived in.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.request_line)?;
        for (name, value) in &self.headers {
            write!(formatter, " | {name}: {value}")?;
        }
        if !self.body.is_empty() {
            write!(formatter, " | body: {}", self.body)?;
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
    /// The request line and every header it carried, with no body. The body
    /// comes after the head on the socket, and [`Self::with_body`] adds it.
    fn parse(head: &str) -> Self {
        let mut lines = head.split(LINE_END);
        let request_line = lines.next().unwrap_or_default().to_string();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
            .collect();

        Self {
            request_line,
            headers,
            body: String::new(),
        }
    }

    /// The length of the body this request states, in bytes.
    ///
    /// A request with no `Content-Length` header has no body. The server does
    /// not read a body that arrives in chunks: reqwest states the length of a
    /// JSON body, and no client in this crate sends one in chunks.
    ///
    /// # Returns
    ///
    /// The stated length, zero when the request states none, or `None` when
    /// the value is not a count or is more than [`MAX_BODY_BYTES`].
    fn body_length(&self) -> Option<usize> {
        match self.header("content-length") {
            None => Some(0),
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|length| *length <= MAX_BODY_BYTES),
        }
    }

    /// This request with `body` as its body.
    ///
    /// # Arguments
    ///
    /// * `body` - The bytes that came after the head. The text is decoded as
    ///   a whole, so a multi-byte character that two reads split stays whole.
    ///
    /// # Returns
    ///
    /// The request, with the body kept as text.
    fn with_body(self, body: &[u8]) -> Self {
        Self {
            body: String::from_utf8_lossy(body).into_owned(),
            ..self
        }
    }

    /// The first line of this request, such as `DELETE /sites/1 HTTP/1.1`.
    ///
    /// The method, the path and the version arrive on one line, and a test
    /// that reads a path also cares which method reached it, so they are
    /// handed back the way the server read them.
    ///
    /// # Returns
    ///
    /// The request line, without the line ending.
    pub fn request_line(&self) -> &str {
        &self.request_line
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

    /// The body of this request, as text.
    ///
    /// A command that changes the state of a controller says what to do in
    /// the body, such as `{"action":"RESTART"}`. A test reads it here to see
    /// the action the controller got.
    ///
    /// # Returns
    ///
    /// The body, or an empty string when the request carried none.
    pub fn body(&self) -> &str {
        &self.body
    }
}

/// An HTTP server for one test.
///
/// The server accepts every connection, records the request head and body,
/// and writes back the one canned response it was built with. It stops when the test
/// drops it.
///
/// The server closes each connection after one answer, and it adds
/// `Connection: close` to each answer, so that a client does not send its
/// next request on the closed socket.
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

    /// Start a server that picks its answer by the request line.
    ///
    /// A command that sends more than one request, such as a listing and then
    /// one request for each item of it, needs a different answer for each.
    ///
    /// # Arguments
    ///
    /// * `routes` - Pairs of a fragment of the request line, such as a part of
    ///   the path, and the raw HTTP response for a request whose line holds
    ///   it. The first pair that matches wins.
    /// * `fallback` - The raw HTTP response for a request that no route
    ///   matches.
    ///
    /// # Returns
    ///
    /// The running server.
    pub async fn routing(routes: &[(String, String)], fallback: &str) -> Self {
        Self::start(Reply::Routes {
            routes: routes.to_vec(),
            fallback: fallback.to_string(),
        })
        .await
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

    /// Start a server that reads each request and closes the connection with
    /// no answer.
    ///
    /// A client gets no HTTP status, the same as from a host that refuses
    /// the connection. A refused connection is not a test a server can hold
    /// safely: a socket that is bound and does not listen drops the packets
    /// on macOS rather than refuses them, and a port that a test frees is a
    /// port that a parallel test can take.
    ///
    /// # Returns
    ///
    /// The running server.
    pub async fn hanging_up() -> Self {
        Self::start(Reply::HangUp).await
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

                let response = match &reply {
                    Reply::Bytes(response) => Some(response),
                    Reply::Routes { routes, fallback } => Some(
                        routes
                            .iter()
                            .find(|(fragment, _)| request.request_line().contains(fragment))
                            .map_or(fallback, |(_, response)| response),
                    ),
                    Reply::Silence | Reply::HangUp => None,
                };

                recorded
                    .lock()
                    .expect("nothing panics while it holds this lock")
                    .push(request);

                match response {
                    Some(response) => {
                        let answer = closing_the_connection(response);
                        let _ = socket.write_all(answer.as_bytes()).await;
                        let _ = socket.flush().await;
                    }
                    // The socket goes out of scope here, which closes the
                    // connection with no answer on it.
                    None if matches!(reply, Reply::HangUp) => {}
                    None => held_open.push(socket),
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

    /// Assert that this server read one request, with `request_line` and
    /// with `body` as its JSON body.
    ///
    /// The body is compared as a parsed document, so the order of the keys
    /// and the white space do not change the result. A key that the body
    /// holds and `body` does not hold fails the assertion, and so does a key
    /// with a `null` value.
    ///
    /// # Arguments
    ///
    /// * `request_line` - The whole request line, such as
    ///   `POST /proxy/network/integration/v1/sites/1/devices/2/actions HTTP/1.1`.
    /// * `body` - The JSON document the request must carry.
    ///
    /// # Panics
    ///
    /// Panics if the server read no request or more than one, if the request
    /// line is different, or if the body is not `body`.
    #[track_caller]
    pub fn assert_one_json_request(&self, request_line: &str, body: &serde_json::Value) {
        let received = self.requests();
        assert_eq!(
            received.len(),
            1,
            "the command must send exactly one request, got {received:?}"
        );

        let request = &received[0];
        assert_eq!(
            request.request_line(),
            request_line,
            "the request must use the right method and path, got {request:?}"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(request.body())
                .ok()
                .as_ref(),
            Some(body),
            "the controller must get the right JSON body, got {request:?}"
        );
    }
}

/// Read one request off `socket`: the head, then the body that the head
/// states the length of.
///
/// The server must read the whole body before it answers. A test reads the
/// body, and a socket that closes with bytes it did not read can reset the
/// connection before the client reads the answer.
///
/// # Arguments
///
/// * `socket` - The accepted connection.
///
/// # Returns
///
/// The request, or `None` when the client sent no complete head, stated a
/// body length the server does not accept, or closed before the whole body
/// arrived.
async fn read_request(socket: &mut TcpStream) -> Option<ReceivedRequest> {
    let mut received = Vec::new();

    let head_end = loop {
        if let Some(end) = received
            .windows(HEAD_END.len())
            .position(|window| window == HEAD_END)
        {
            break end;
        }
        if received.len() > MAX_HEAD_BYTES {
            return None;
        }
        read_chunk(socket, &mut received).await?;
    };

    let request = ReceivedRequest::parse(&String::from_utf8_lossy(&received[..head_end]));

    // The first reads can hold part of the body, or all of it, after the
    // end of the head.
    let body_start = head_end + HEAD_END.len();
    let body_end = body_start + request.body_length()?;
    while received.len() < body_end {
        read_chunk(socket, &mut received).await?;
    }

    Some(request.with_body(&received[body_start..body_end]))
}

/// `response` with a header that says the server closes the connection after
/// it writes this answer.
///
/// The server closes each connection after one answer. A client that is not
/// told keeps the connection in its pool, and sometimes sends the next request
/// on the socket that the server already closed. The server then records one
/// request too few, and a test that counts the requests fails at random.
///
/// The server adds the header where it writes each answer. Thus every answer
/// gets it: the answer from each builder, each route, the fallback, and a
/// response that a test writes by hand.
///
/// # Arguments
///
/// * `response` - The raw HTTP response, as the test gave it.
///
/// # Returns
///
/// The response with the header directly after the status line. A response
/// with no line ending has no head for the header, and comes back unchanged.
fn closing_the_connection(response: &str) -> String {
    match response.split_once(LINE_END) {
        Some((status_line, rest)) => {
            format!("{status_line}{LINE_END}{CONNECTION_CLOSE}{LINE_END}{rest}")
        }
        None => response.to_string(),
    }
}

/// Read the next bytes off `socket` onto the end of `received`.
///
/// # Arguments
///
/// * `socket` - The accepted connection.
/// * `received` - The bytes read so far.
///
/// # Returns
///
/// `Some` when bytes arrived, or `None` when the client closed the connection
/// or the read failed.
async fn read_chunk(socket: &mut TcpStream, received: &mut Vec<u8>) -> Option<()> {
    let mut chunk = [0_u8; READ_CHUNK_BYTES];
    let read = socket.read(&mut chunk).await.ok()?;
    if read == 0 {
        return None;
    }
    received.extend_from_slice(&chunk[..read]);
    Some(())
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
    json_response_with_status("200 OK", body)
}

/// The response that carries `body` as its JSON document, with `status`.
///
/// # Arguments
///
/// * `status` - The status code and reason, such as `404 Not Found`.
/// * `body` - The JSON document to answer with.
///
/// # Returns
///
/// A raw response with that status that carries `body`.
pub fn json_response_with_status(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
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

/// How much of a body one chunk of a chunked response carries.
///
/// Two chunks prove the framing. A reader that stops at the first chunk, or
/// that counts the chunk headers as body, fails a test that spans two.
const CHUNKS_PER_BODY: usize = 2;

/// The response that carries `body` in chunks and states no length.
///
/// A chunked response is the shape a `Content-Length` check cannot see: the
/// server states no size at all, and the reader learns the size only from
/// what arrives.
///
/// # Arguments
///
/// * `body` - The document to answer with.
///
/// # Returns
///
/// A raw `200 OK` response that carries `body` as chunks.
pub fn chunked_response(body: &str) -> String {
    let mut response = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n",
    );

    // The split lands on a character boundary, so a multi-byte character is
    // never cut in half by the framing.
    let halfway = body.len() / CHUNKS_PER_BODY;
    let split = (halfway..=body.len())
        .find(|index| body.is_char_boundary(*index))
        .unwrap_or(body.len());
    let (first, second) = body.split_at(split);

    for chunk in [first, second] {
        if !chunk.is_empty() {
            response.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
        }
    }
    response.push_str("0\r\n\r\n");

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::UnifiClient;
    use std::time::Duration;

    /// The API key a test hands the client. Nothing reads it back.
    const API_KEY: &str = "an-api-key";

    /// How long a test waits for the server to finish an answer.
    ///
    /// The server closes each connection after it writes the answer, so a
    /// read to the end of the stream ends at once. The bound only stops a
    /// server that keeps the connection open from holding the test forever.
    const ANSWER_PATIENCE: Duration = Duration::from_secs(30);

    /// The header name that tells a client what happens to the connection
    /// after this answer.
    const CONNECTION_HEADER: &str = "connection";

    /// The value of the `Connection` header that says the server closes the
    /// connection after this answer.
    const CLOSE: &str = "close";

    /// An HTTP answer, cut at its framing.
    #[derive(Debug, PartialEq)]
    struct Framed<'a> {
        /// The first line, such as `HTTP/1.1 200 OK`.
        status_line: &'a str,
        /// Every header line, in the order the answer gives them.
        headers: Vec<&'a str>,
        /// Everything after the empty line that ends the head.
        body: &'a str,
    }

    /// Cut `raw` at its framing.
    ///
    /// # Arguments
    ///
    /// * `raw` - A whole HTTP answer, head and body.
    ///
    /// # Returns
    ///
    /// The status line, the header lines and the body.
    ///
    /// # Panics
    ///
    /// Panics if `raw` has no empty line after its head.
    fn framed(raw: &str) -> Framed<'_> {
        let head_end = String::from_utf8_lossy(HEAD_END);
        let (head, body) = raw
            .split_once(head_end.as_ref())
            .expect("an HTTP answer has an empty line after its head");
        let mut lines = head.split(LINE_END);

        Framed {
            status_line: lines.next().unwrap_or_default(),
            headers: lines.collect(),
            body,
        }
    }

    /// The value of the header `line`, when its name is `Connection`.
    ///
    /// # Arguments
    ///
    /// * `line` - One header line, such as `Connection: close`.
    ///
    /// # Returns
    ///
    /// The value in lower case, or `None` when the line is another header.
    fn connection_value(line: &str) -> Option<String> {
        line.split_once(':')
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case(CONNECTION_HEADER))
            .map(|(_, value)| value.trim().to_ascii_lowercase())
    }

    /// Send `GET path` to `server` on a plain socket, and read the answer up
    /// to the end of the stream.
    ///
    /// A plain socket shows the bytes exactly as the server wrote them.
    ///
    /// # Arguments
    ///
    /// * `server` - The server to ask.
    /// * `path` - The path of the request.
    ///
    /// # Returns
    ///
    /// The whole answer, as text.
    ///
    /// # Panics
    ///
    /// Panics if the server refuses the connection, keeps it open for longer
    /// than [`ANSWER_PATIENCE`], or writes an answer that is not text.
    async fn raw_answer(server: &TestServer, path: &str) -> String {
        let address = server
            .origin()
            .strip_prefix("http://")
            .expect("a test server origin starts with http://");
        let mut socket = TcpStream::connect(address)
            .await
            .expect("a test server accepts a connection");
        socket
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
            .await
            .expect("a test server reads the request");

        let mut answer = Vec::new();
        tokio::time::timeout(ANSWER_PATIENCE, socket.read_to_end(&mut answer))
            .await
            .expect("the server must close the connection after it answers")
            .expect("the answer must arrive");

        String::from_utf8(answer).expect("every answer a test server writes is text")
    }

    /// Assert that `answer` is `sent` with one `Connection: close` header
    /// added, and with no other change.
    ///
    /// # Arguments
    ///
    /// * `case` - The name of the case, for the message of a failure.
    /// * `sent` - The response the test gave the server.
    /// * `answer` - The bytes the server wrote.
    ///
    /// # Panics
    ///
    /// Panics if the answer has no `Connection: close` header, has more than
    /// one `Connection` header, or differs from `sent` in any other way.
    #[track_caller]
    fn assert_says_it_closes(case: &str, sent: &str, answer: &str) {
        let written = framed(answer);
        let (connection, others): (Vec<&str>, Vec<&str>) = written
            .headers
            .iter()
            .partition(|line| connection_value(line).is_some());

        assert_eq!(
            connection
                .iter()
                .filter_map(|line| connection_value(line))
                .collect::<Vec<_>>(),
            vec![CLOSE.to_string()],
            "{case}: the answer must say once that the server closes the connection, got {answer:?}"
        );
        assert_eq!(
            Framed {
                status_line: written.status_line,
                headers: others,
                body: written.body,
            },
            framed(sent),
            "{case}: the server must write the rest of the answer as the test gave it"
        );
    }

    /// The server closes each connection after it answers, and every answer
    /// must say so. A client that is not told keeps the connection for its
    /// next request, and sometimes sends that request on the socket the
    /// server already closed. The server then records one request too few,
    /// and a test that counts requests fails at random.
    #[tokio::test]
    async fn every_answer_says_that_the_server_closes_the_connection() {
        let cases = [
            ("json_response", json_response(r#"{"answer":1}"#)),
            (
                "json_response_with_status",
                json_response_with_status("404 Not Found", r#"{"message":"Not Found"}"#),
            ),
            ("empty_json", empty_json()),
            ("redirect_to", redirect_to("http://127.0.0.1:9/moved")),
            (
                "chunked_response",
                chunked_response(r#"{"name":"日本語 🎉 café"}"#),
            ),
            (
                "a response a test writes by hand",
                "HTTP/1.1 204 No Content\r\n\r\n".to_string(),
            ),
        ];

        for (case, response) in cases {
            let server = TestServer::replying(&response).await;

            let answer = raw_answer(&server, "/info").await;

            assert_says_it_closes(case, &response, &answer);
        }
    }

    /// A server that routes picks one of several responses. Each of them,
    /// and the fallback, must say that the server closes the connection.
    #[tokio::test]
    async fn every_route_and_the_fallback_say_that_the_server_closes_the_connection() {
        let routed = json_response(r#"{"route":"matched"}"#);
        let fallback = json_response_with_status("404 Not Found", r#"{"route":"none"}"#);
        let server =
            TestServer::routing(&[("/routed".to_string(), routed.clone())], &fallback).await;

        for (case, path, response) in [
            ("a matched route", "/routed", &routed),
            ("the fallback", "/elsewhere", &fallback),
        ] {
            let answer = raw_answer(&server, path).await;

            assert_says_it_closes(case, response, &answer);
        }
    }

    /// Send `body` as the JSON body of a `POST` to a new server.
    ///
    /// # Arguments
    ///
    /// * `body` - The document the client sends.
    ///
    /// # Returns
    ///
    /// The one request the server read.
    async fn post_to_a_server(body: &serde_json::Value) -> ReceivedRequest {
        let server = TestServer::replying(&empty_json()).await;
        let client = UnifiClient::new(server.origin(), API_KEY, false)
            .expect("a loopback URL must build a client");

        let _: serde_json::Value = client
            .post("sites/a-site/devices/a-device/actions", body)
            .await
            .expect("the server answered the request");

        let received = server.requests();
        assert_eq!(
            received.len(),
            1,
            "one post is one request, got {received:?}"
        );
        received[0].clone()
    }

    /// A test of a command that changes the state of a controller reads the
    /// body the controller got. The server keeps that body, so the test can
    /// read it.
    #[tokio::test]
    async fn a_posted_json_body_is_kept_as_the_client_sent_it() {
        let sent = serde_json::json!({ "action": "RESTART" });

        let request = post_to_a_server(&sent).await;

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(request.body()).ok(),
            Some(sent),
            "the server must keep the body it read, got {request:?}"
        );
    }

    /// A body that arrives over more than one read is kept whole. The body
    /// holds multi-byte characters, so a server that counts characters where
    /// `Content-Length` counts bytes stops at the wrong place.
    #[tokio::test]
    async fn a_body_longer_than_one_read_is_kept_whole() {
        let sent = serde_json::json!({ "name": "日本語 🎉 café ".repeat(200) });
        let sent_bytes = sent.to_string().len();
        assert!(
            sent_bytes > READ_CHUNK_BYTES * 2,
            "the body must span more than one read to test the reads"
        );

        let request = post_to_a_server(&sent).await;

        // The body is too long to print in full, so the message gives the
        // counts of bytes.
        let kept = serde_json::from_str::<serde_json::Value>(request.body()).ok();
        assert!(
            kept.as_ref() == Some(&sent),
            "the server must keep every byte of a long body, kept {} of {sent_bytes} bytes",
            request.body().len()
        );
    }
}
