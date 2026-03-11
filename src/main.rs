use std::env;
use std::error::Error;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use git2::{Repository, Status, StatusOptions};
use nanoserde::SerJson;
use notify::{RecursiveMode, Watcher};
use tungstenite::Message;

const HTTP_ADDR: &str = "127.0.0.1:8080";
const WS_ADDR: &str = "127.0.0.1:8081";
const APP_URL: &str = "http://127.0.0.1:8080/";

#[derive(SerJson, Clone)]
struct GitStatusEntry {
    path: String,
    code: String,
}

#[derive(SerJson, Clone)]
struct GitStatusSnapshot {
    repo_path: String,
    branch: String,
    clean: bool,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    updated_unix_ms: u64,
    error: String,
    entries: Vec<GitStatusEntry>,
}

struct AppState {
    repo_path: PathBuf,
    current_status_json: Mutex<String>,
    clients: Mutex<Vec<(usize, mpsc::Sender<String>)>>,
    next_client_id: AtomicUsize,
}

fn parse_repo_path_from_args() -> Result<PathBuf, Box<dyn Error>> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "visual-git".to_string());
    let raw = args
        .next()
        .ok_or_else(|| format!("usage: {} <repo-path>", program))?;
    let path = PathBuf::from(raw);
    Ok(path.canonicalize().unwrap_or(path))
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn status_code(status: Status) -> String {
    if status.is_conflicted() {
        return "UU".to_string();
    }

    let index = if status.is_index_new() {
        'A'
    } else if status.is_index_modified() {
        'M'
    } else if status.is_index_deleted() {
        'D'
    } else if status.is_index_renamed() {
        'R'
    } else if status.is_index_typechange() {
        'T'
    } else {
        ' '
    };

    let worktree = if status.is_wt_new() {
        '?'
    } else if status.is_wt_modified() {
        'M'
    } else if status.is_wt_deleted() {
        'D'
    } else if status.is_wt_renamed() {
        'R'
    } else if status.is_wt_typechange() {
        'T'
    } else {
        ' '
    };

    format!("{}{}", index, worktree)
}

fn build_status_snapshot(repo_path: &Path) -> GitStatusSnapshot {
    let mut snapshot = GitStatusSnapshot {
        repo_path: repo_path.display().to_string(),
        branch: "DETACHED".to_string(),
        clean: true,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        updated_unix_ms: now_unix_ms(),
        error: String::new(),
        entries: Vec::new(),
    };

    let repo = match Repository::open(repo_path) {
        Ok(repo) => repo,
        Err(err) => {
            snapshot.error = format!("failed to open repo: {}", err);
            snapshot.clean = false;
            return snapshot;
        }
    };

    snapshot.branch = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(str::to_string))
        .unwrap_or_else(|| "DETACHED".to_string());

    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let statuses = match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses,
        Err(err) => {
            snapshot.error = format!("failed to query status: {}", err);
            snapshot.clean = false;
            return snapshot;
        }
    };

    for entry in statuses.iter() {
        let status = entry.status();
        let code = status_code(status);
        if code == "  " {
            continue;
        }

        if status.is_conflicted() {
            snapshot.staged += 1;
            snapshot.unstaged += 1;
        } else {
            if status.is_index_new()
                || status.is_index_modified()
                || status.is_index_deleted()
                || status.is_index_renamed()
                || status.is_index_typechange()
            {
                snapshot.staged += 1;
            }

            if status.is_wt_modified()
                || status.is_wt_deleted()
                || status.is_wt_renamed()
                || status.is_wt_typechange()
            {
                snapshot.unstaged += 1;
            }

            if status.is_wt_new() {
                snapshot.untracked += 1;
            }
        }

        snapshot.entries.push(GitStatusEntry {
            path: entry.path().unwrap_or("<unknown>").to_string(),
            code,
        });
    }

    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.clean = snapshot.entries.is_empty() && snapshot.error.is_empty();
    snapshot
}

fn build_status_json(repo_path: &Path) -> String {
    build_status_snapshot(repo_path).serialize_json()
}

fn broadcast_status(state: &AppState, json: &str) {
    let mut clients = state.clients.lock().unwrap();
    clients.retain(|(_, tx)| tx.send(json.to_string()).is_ok());
}

fn refresh_and_broadcast(state: &AppState) {
    let next_json = build_status_json(&state.repo_path);
    let mut current = state.current_status_json.lock().unwrap();
    if *current == next_json {
        return;
    }
    *current = next_json.clone();
    drop(current);
    broadcast_status(state, &next_json);
}

fn read_http_request(stream: &mut TcpStream) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut buf = vec![0u8; 8192];
    let mut pos = 0;

    loop {
        let n = stream.read(&mut buf[pos..])?;
        if n == 0 {
            return Err("connection closed while reading request".into());
        }
        pos += n;

        if buf[..pos].windows(4).any(|w| w == b"\r\n\r\n") {
            buf.truncate(pos);
            return Ok(buf);
        }

        if pos == buf.len() {
            return Err("request too large".into());
        }
    }
}

fn parse_request_line(buf: &[u8]) -> Result<(String, String), Box<dyn Error>> {
    let request = std::str::from_utf8(buf)?;
    let mut lines = request.split("\r\n");
    let request_line = lines.next().ok_or("missing request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing method")?.to_string();
    let path = parts.next().ok_or("missing path")?.to_string();
    Ok((method, path))
}

fn write_http_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), Box<dyn Error>> {
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        content_type,
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

fn serve_http(mut stream: TcpStream) -> Result<(), Box<dyn Error>> {
    let req_buf = read_http_request(&mut stream)?;
    let (method, path) = parse_request_line(&req_buf)?;
    println!("{} {}", method, path);

    if path == "/" {
        let body = include_str!("../index.html");
        write_http_response(&mut stream, "200 OK", "text/html; charset=utf-8", body)?;
        return Ok(());
    }

    write_http_response(
        &mut stream,
        "404 Not Found",
        "text/plain; charset=utf-8",
        "not found",
    )?;
    Ok(())
}

fn handle_http_connection(stream: TcpStream) {
    if let Err(e) = serve_http(stream) {
        eprintln!("http connection error: {}", e);
    }
}

fn serve_ws_client(stream: TcpStream, state: Arc<AppState>) -> Result<(), Box<dyn Error>> {
    let mut ws = tungstenite::accept(stream)?;
    ws.get_mut().set_nonblocking(true)?;
    let (tx, rx) = mpsc::channel::<String>();
    let client_id = state.next_client_id.fetch_add(1, Ordering::Relaxed);

    {
        let mut clients = state.clients.lock().unwrap();
        clients.push((client_id, tx));
    }

    let initial = { state.current_status_json.lock().unwrap().clone() };
    ws.send(Message::Text(initial.into()))?;

    'client: loop {
        loop {
            match rx.try_recv() {
                Ok(next_status) => ws.send(Message::Text(next_status.into()))?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'client,
            }
        }

        match ws.read() {
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(payload)) => ws.send(Message::Pong(payload))?,
            Ok(_) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(tungstenite::Error::Io(err)) if err.kind() == ErrorKind::WouldBlock => {}
            Err(err) => return Err(err.into()),
        }

        thread::sleep(Duration::from_millis(100));
    }

    let mut clients = state.clients.lock().unwrap();
    clients.retain(|(id, _)| *id != client_id);
    Ok(())
}

fn handle_ws_connection(stream: TcpStream, state: Arc<AppState>) {
    if let Err(e) = serve_ws_client(stream, state) {
        eprintln!("websocket error: {}", e);
    }
}

fn run_http_server(listener: TcpListener) -> Result<(), Box<dyn Error>> {
    println!("http listening on {}", HTTP_ADDR);
    loop {
        let (stream, _) = listener.accept()?;
        thread::spawn(move || {
            handle_http_connection(stream);
        });
    }
}

fn run_ws_server(listener: TcpListener, state: Arc<AppState>) -> Result<(), Box<dyn Error>> {
    println!("websocket listening on {}", WS_ADDR);
    loop {
        let (stream, _) = listener.accept()?;
        let state_for_client = Arc::clone(&state);
        thread::spawn(move || {
            handle_ws_connection(stream, state_for_client);
        });
    }
}

fn run_repo_watcher(repo_path: PathBuf, state: Arc<AppState>) -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })?;

    watcher.watch(&repo_path, RecursiveMode::Recursive)?;
    println!("watching repo at {}", repo_path.display());

    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                while let Ok(Ok(_)) = rx.recv_timeout(Duration::from_millis(75)) {}
                refresh_and_broadcast(&state);
            }
            Ok(Err(err)) => eprintln!("watch error: {}", err),
            Err(_) => break,
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let repo_path = parse_repo_path_from_args()?;
    Repository::open(&repo_path)?;

    let initial_status_json = build_status_json(&repo_path);
    let state = Arc::new(AppState {
        repo_path: repo_path.clone(),
        current_status_json: Mutex::new(initial_status_json),
        clients: Mutex::new(Vec::new()),
        next_client_id: AtomicUsize::new(1),
    });

    let http_listener = TcpListener::bind(HTTP_ADDR)?;
    let ws_listener = TcpListener::bind(WS_ADDR)?;

    if let Err(e) = webbrowser::open(APP_URL) {
        eprintln!("failed to open browser: {}", e);
    }

    {
        let state_for_ws = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(e) = run_ws_server(ws_listener, state_for_ws) {
                eprintln!("websocket server error: {}", e);
            }
        });
    }

    {
        let state_for_watcher = Arc::clone(&state);
        thread::spawn(move || {
            if let Err(e) = run_repo_watcher(repo_path, state_for_watcher) {
                eprintln!("watcher error: {}", e);
            }
        });
    }

    run_http_server(http_listener)
}
