use std::env;
use std::error::Error;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use nanoserde::{DeJson, SerJson};
use notify::{RecursiveMode, Watcher};

mod git;
mod http;
mod router;
mod ui_assets;
mod util;

const HTTP_ADDR: &str = "127.0.0.1:8080";
const APP_URL: &str = "http://127.0.0.1:8080/";

#[derive(SerJson)]
struct CommandResponse {
    request_id: String,
    ok: bool,
    error: String,
}

struct AppState {
    repo_path: PathBuf,
    backend: Arc<dyn git::Backend>,
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

fn build_status_json(state: &AppState) -> String {
    state.backend.read_status(&state.repo_path).serialize_json()
}

fn broadcast_status(state: &AppState, json: &str) {
    let mut clients = state.clients.lock().unwrap();
    clients.retain(|(_, tx)| tx.send(json.to_string()).is_ok());
}

fn refresh_and_broadcast(state: &AppState) {
    let next_json = build_status_json(state);
    let mut current = state.current_status_json.lock().unwrap();
    if *current == next_json {
        return;
    }
    *current = next_json.clone();
    drop(current);
    broadcast_status(state, &next_json);
}

fn serve_http(
    mut stream: TcpStream,
    state: Arc<AppState>,
) -> Result<(), Box<dyn Error>> {
    let request = http::read_http_request(&mut stream)?;
    let path = http::normalize_path(&request.path);
    println!("{} {}", request.method, request.path);

    let router = router::new();

    if request.method == "GET" && path == "/events" {
        return http::serve_sse_client(stream, state);
    }

    if request.method == "POST" && path == "/command" {
        let body = std::str::from_utf8(&request.body).unwrap_or("");
        let command = match git::CommandRequest::deserialize_json(body) {
            Ok(command) => command,
            Err(err) => {
                let resp = CommandResponse {
                    request_id: String::new(),
                    ok: false,
                    error: format!("invalid command payload: {}", err),
                }
                .serialize_json();
                http::write_json_response(
                    &mut stream,
                    "400 Bad Request",
                    &resp,
                )?;
                return Ok(());
            }
        };

        if command.request_id.is_empty() {
            let resp = CommandResponse {
                request_id: String::new(),
                ok: false,
                error: "request_id must not be empty".to_string(),
            }
            .serialize_json();
            http::write_json_response(&mut stream, "400 Bad Request", &resp)?;
            return Ok(());
        }

        let result = state.backend.run_command(&state.repo_path, &command);
        if result.ok && command.kind == "refresh_status" {
            refresh_and_broadcast(&state);
        }

        let resp = CommandResponse {
            request_id: command.request_id,
            ok: result.ok,
            error: result.error,
        }
        .serialize_json();
        http::write_json_response(&mut stream, "200 OK", &resp)?;
        return Ok(());
    }

    if request.method == "GET" {
        let asset_path = if path == "/" { "/index.html" } else { path };
        if let Some(asset_body) = ui_assets::get(asset_path) {
            let content_type = http::content_type_for_path(asset_path);
            http::write_http_response_bytes(
                &mut stream,
                "200 OK",
                content_type,
                asset_body,
            )?;
            return Ok(());
        }

        if path == "/" && !ui_assets::has_assets() {
            http::write_http_response_bytes(
                &mut stream,
                "503 Service Unavailable",
                "text/html; charset=utf-8",
                ui_assets::missing_assets_html(),
            )?;
            return Ok(());
        }
    }

    http::write_http_response(
        &mut stream,
        "404 Not Found",
        "text/plain; charset=utf-8",
        "not found",
    )?;
    Ok(())
}

fn handle_http_connection(stream: TcpStream, state: Arc<AppState>) {
    if let Err(e) = serve_http(stream, state) {
        eprintln!("http connection error: {}", e);
    }
}

fn run_http_server(
    listener: TcpListener,
    state: Arc<AppState>,
) -> Result<(), Box<dyn Error>> {
    println!("http listening on {}", HTTP_ADDR);
    loop {
        let (stream, _) = listener.accept()?;
        let state_for_client = Arc::clone(&state);
        thread::spawn(move || {
            handle_http_connection(stream, state_for_client);
        });
    }
}

fn run_repo_watcher(
    repo_path: PathBuf,
    state: Arc<AppState>,
) -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })?;

    watcher.watch(&repo_path, RecursiveMode::Recursive)?;
    println!("watching repo at {}", repo_path.display());

    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                while let Ok(Ok(_)) = rx.recv_timeout(Duration::from_millis(75))
                {
                }
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
    let backend: Arc<dyn git::Backend> = Arc::new(git::CliBackend::new());

    let initial_snapshot = backend.read_status(&repo_path);
    if !initial_snapshot.error.is_empty() {
        return Err(initial_snapshot.error.into());
    }

    let state = Arc::new(AppState {
        repo_path: repo_path.clone(),
        backend,
        current_status_json: Mutex::new(initial_snapshot.serialize_json()),
        clients: Mutex::new(Vec::new()),
        next_client_id: AtomicUsize::new(1),
    });

    let http_listener = TcpListener::bind(HTTP_ADDR)?;

    {
        thread::spawn(move || {
            if let Err(e) = util::try_open_browser(APP_URL) {
                eprintln!("failed to open browser: {}", e);
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

    run_http_server(http_listener, state)
}
