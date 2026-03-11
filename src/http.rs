//! Minimal plumbing needed to serve the app over HTTP.
//! All the app needs to do is differentiate by path and read request bodies.
use std::{
    error::Error,
    io::{Read, Write},
    net::TcpStream,
    sync::{Arc, atomic::Ordering, mpsc},
    time::Duration,
};

use crate::AppState;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;

pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

pub fn normalize_path(raw_path: &str) -> &str {
    raw_path.split('?').next().unwrap_or(raw_path)
}

pub fn read_http_request(
    stream: &mut TcpStream,
) -> Result<HttpRequest, Box<dyn Error>> {
    let mut buf = Vec::with_capacity(4096);
    let mut header_end = None;

    loop {
        let mut chunk = [0u8; 2048];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err("connection closed while reading request".into());
        }
        buf.extend_from_slice(&chunk[..n]);

        if header_end.is_none() {
            if let Some(end) = find_header_end(&buf) {
                header_end = Some(end);
            } else if buf.len() > MAX_HEADER_BYTES {
                return Err("request headers too large".into());
            }
        }

        if header_end.is_some() {
            break;
        }
    }

    let header_end = header_end.ok_or("malformed HTTP request")?;
    let header_text = std::str::from_utf8(&buf[..header_end])?;
    let mut lines = header_text.split("\r\n");

    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_string();
    let path = parts.next().ok_or("missing path")?.to_string();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>()?;
                if content_length > MAX_BODY_BYTES {
                    return Err("request body too large".into());
                }
            }
        }
    }

    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk =
            vec![0u8; content_length.saturating_sub(body.len()).min(4096)];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err("connection closed while reading body".into());
        }
        body.extend_from_slice(&chunk[..n]);
    }

    body.truncate(content_length);

    Ok(HttpRequest { method, path, body })
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

pub fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    write_http_response_bytes(stream, status, content_type, body.as_bytes())
}

pub fn write_http_response_bytes(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> Result<(), Box<dyn Error>> {
    let headers = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len(),
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

pub fn write_json_response(
    stream: &mut TcpStream,
    status: &str,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    write_http_response(stream, status, "application/json; charset=utf-8", body)
}

pub fn content_type_for_path(path: &str) -> &'static str {
    let ext = path
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

pub fn serve_sse_client(
    mut stream: TcpStream,
    state: Arc<AppState>,
) -> Result<(), Box<dyn Error>> {
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<String>();

    {
        let mut clients = state.clients.lock().unwrap();
        clients.push((client_id, tx));
    }

    let headers = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n";
    stream.write_all(headers.as_bytes())?;

    let initial = state.current_status_json.lock().unwrap().clone();
    let initial_message = format!("data: {}\n\n", initial);
    stream.write_all(initial_message.as_bytes())?;
    stream.flush()?;

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
