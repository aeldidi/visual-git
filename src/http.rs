//! Minimal plumbing needed to serve the app over HTTP.
//! All the app needs to do is differentiate by path and read request bodies.
use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::{Read, Write},
    net::TcpStream,
    str::FromStr,
    sync::{Arc, atomic::Ordering, mpsc},
    time::Duration,
};

use nanoserde::SerJson;

use crate::{
    AppState, bail,
    dynerror::{self, Context},
    err,
};

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub struct Request {
    pub method: Method,
    pub path: String,
    pub body: Vec<u8>,
}

/// An HTTP method.
#[derive(Hash, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl FromStr for Method {
    type Err = dynerror::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "GET" => Ok(Method::Get),
            "POST" => Ok(Method::Post),
            _ => Err(err!("unrecognized method: {}", s)),
        }
    }
}

impl Display for Method {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Method::Get => write!(f, "GET"),
            Method::Post => write!(f, "POST"),
        }
    }
}

/// an HTTP status code.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StatusCode(i16);

impl StatusCode {
    /// 200 OK
    /// The request succeeded.
    pub const OK: StatusCode = StatusCode(200);
    /// 400 Bad Request
    /// The server cannot or will not process the request due to something that
    /// is perceived to be a client error.
    pub const BAD_REQUEST: StatusCode = StatusCode(400);
    /// 404 Not Found
    /// The server cannot find the requested resource.
    pub const NOT_FOUND: StatusCode = StatusCode(404);
    /// 500 Internal Server Error
    /// The server has encountered a situation it does not know how to handle.
    pub const INTERNAL_SERVER_ERROR: StatusCode = StatusCode(500);
    /// 405 Method Not Allowed
    /// The request method is known by the server but is not supported by the
    /// target resource.
    pub const METHOD_NOT_ALLOWED: StatusCode = StatusCode(405);
}

impl Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            StatusCode::OK => write!(f, "200 OK"),
            StatusCode::BAD_REQUEST => write!(f, "400 Bad Request"),
            StatusCode::NOT_FOUND => write!(f, "404 Not Found"),
            StatusCode::INTERNAL_SERVER_ERROR => {
                write!(f, "500 Internal Server Error")
            }
            StatusCode::METHOD_NOT_ALLOWED => {
                write!(f, "405 Method Not Allowed")
            }
            _ => write!(f, "{}", self.0),
        }
    }
}

/// A response returned from an HTTP endpoint.
pub struct Response {
    status: StatusCode,
    headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

impl Response {
    /// Returns an HTTP response with a given status code, allowing.
    pub fn builder(status: StatusCode) -> Self {
        Self {
            status,
            headers: HashMap::new(),
            body: None,
        }
    }

    /// Adds a new header to a response.
    pub fn header(mut self, key: String, value: String) -> Self {
        self.headers.insert(key, value);
        self
    }

    /// Adds a body to the response.
    pub fn body(self, body: impl AsRef<[u8]>) -> Self {
        Self {
            status: self.status,
            headers: self.headers,
            body: Some(body.as_ref().to_vec()),
        }
    }
}

/// Returns a response which means the server encountered an error.
pub fn internal_server_error(msg: String) -> Response {
    Response {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        headers: HashMap::new(),
        body: Some(msg.into()),
    }
}

/// Return a JSON response with a 200 response code.
pub fn json<T: SerJson>(value: T) -> Response {
    let headers = HashMap::<String, String>::from([(
        "Content-Type".into(),
        "application/json".into(),
    )]);
    Response {
        status: StatusCode(200),
        headers: headers,
        body: Some(value.serialize_json().into()),
    }
}

/// Returns an ok response (200).
pub fn ok() -> Response {
    Response {
        status: StatusCode::OK,
        headers: HashMap::new(),
        body: None,
    }
}

/// Returns a "Method Not Allowed" response (405).
pub fn method_not_allowed() -> Response {
    Response {
        status: StatusCode::METHOD_NOT_ALLOWED,
        headers: HashMap::new(),
        body: None,
    }
}

/// Returns a "Not Found" response (404).
pub fn not_found() -> Response {
    Response {
        status: StatusCode::NOT_FOUND,
        headers: HashMap::new(),
        body: None,
    }
}

pub fn normalize_path(raw_path: &str) -> &str {
    raw_path.split('?').next().unwrap_or(raw_path)
}

pub fn read_http_request(stream: &mut TcpStream) -> dynerror::Result<Request> {
    let mut buf = Vec::with_capacity(4096);
    let mut header_end = None;

    loop {
        let mut chunk = [0u8; 2048];
        let n = stream
            .read(&mut chunk)
            .context("couldn't read chunk from TCP stream")?;
        if n == 0 {
            bail!("connection closed while reading request");
        }
        buf.extend_from_slice(&chunk[..n]);

        if header_end.is_none() {
            if let Some(end) = find_header_end(&buf) {
                header_end = Some(end);
            } else if buf.len() > MAX_HEADER_BYTES {
                bail!("request headers too large");
            }
        }

        if header_end.is_some() {
            break;
        }
    }

    let header_end = header_end
        .ok_or(err!("request missing end of line following headers"))?;
    let header_text = str::from_utf8(&buf[..header_end])
        .context("invalid UTF-8 in request headers")?;
    let mut lines = header_text.split("\r\n");

    let request_line =
        lines.next().ok_or(err!("request has no request-line"))?;
    let mut parts = request_line.split_whitespace();
    let method_str = parts.next().ok_or(err!("request is missing a method"))?;
    let method = method_str
        .parse()
        .context(format!("invalid request method: {}", method_str))?;
    let path = parts
        .next()
        .ok_or(err!("request is missing a path"))?
        .to_string();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value
                    .trim()
                    .parse::<usize>()
                    .context("couldn't parse content-length")?;
                if content_length > MAX_BODY_BYTES {
                    bail!("request body too large");
                }
            }
        }
    }

    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk =
            vec![0u8; content_length.saturating_sub(body.len()).min(4096)];
        let n = stream
            .read(&mut chunk)
            .context("failed to read request body")?;
        if n == 0 {
            bail!("connection closed while reading body");
        }
        body.extend_from_slice(&chunk[..n]);
    }

    body.truncate(content_length);

    Ok(Request { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn write_response(
    stream: &mut TcpStream,
    resp: Response,
) -> dynerror::Result<()> {
    let content_type = resp
        .headers
        .get("Content-Type".into())
        .map(Clone::clone)
        .unwrap_or("application/octet-stream".into());
    let headers = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        resp.status,
        content_type,
        resp.body.as_ref().map(|x| x.len()).unwrap_or(0),
    );
    stream
        .write_all(headers.as_bytes())
        .context("failed to write response headers")?;
    if let Some(x) = resp.body.as_ref() {
        stream
            .write_all(x)
            .context("failed to write response body")?;
    }
    Ok(())
}

pub fn serve_sse_client(
    mut stream: TcpStream,
    state: Arc<AppState>,
) -> dynerror::Result<()> {
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<String>();

    {
        let mut clients = state.clients.lock().unwrap();
        clients.push((client_id, tx));
    }

    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
    stream
        .write_all(headers.as_bytes())
        .context("failed to write headers")?;

    {
        let initial = state.current_status.lock().unwrap().clone();
        let initial_message = format!("data: {}\n\n", initial.serialize_json());
        stream
            .write_all(initial_message.as_bytes())
            .context("failed to write initial message")?;
        stream.flush().context("failed to flush initial message")?;
    }

    loop {
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(next_json) => {
                let message = format!("data: {}\n\n", next_json);
                if stream.write_all(message.as_bytes()).is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stream.write_all(b": keep-alive\n\n").is_err() {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if stream.flush().is_err() {
            break;
        }
    }

    let mut clients = state.clients.lock().unwrap();
    clients.retain(|(id, _)| *id != client_id);
    Ok(())
}
