use std::env;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use nanoserde::SerJson;
use notify::{RecursiveMode, Watcher};

use crate::dynerror::Context;

mod dynerror;
mod git;
mod http;
mod router;
mod routes;
mod ui_assets;
mod util;

const HTTP_ADDR: &str = "127.0.0.1:8080";
const APP_URL: &str = "http://127.0.0.1:8080/";

struct AppState {
    repo_path: PathBuf,
    backend: Arc<dyn git::Backend>,
    current_status: Mutex<git::StatusSnapshot>,
    clients: Mutex<Vec<(usize, mpsc::Sender<String>)>>,
    next_client_id: AtomicUsize,
}

fn parse_repo_path_from_args() -> dynerror::Result<PathBuf> {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "visual-git".to_string());
    let raw = args
        .next()
        .ok_or_else(|| err!("usage: {} <repo-path>", program))?;
    let path = PathBuf::from(raw);
    Ok(path.canonicalize().unwrap_or(path))
}

fn serve_http(
    mut stream: TcpStream,
    state: Arc<AppState>,
) -> dynerror::Result<()> {
    let request = http::read_http_request(&mut stream)?;
    let path = http::normalize_path(&request.path);
    println!("{} {}", request.method, request.path);

    // Special case this since it constantly streams a response.
    if request.method == http::Method::Get && path == "/events" {
        return http::serve_sse_client(stream, state);
    }

    let router =
        router::new(state).post("/refresh", routes::refresh::refresh_status);

    match router.handle(path.to_string(), request) {
        Ok(resp) => http::write_response(&mut stream, resp)?,
        // TODO: write error response
        Err(err) => http::write_response(
            &mut stream,
            http::Response::builder(http::StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type".into(), "text/plain".into())
                .body(err.to_string()),
        )?,
    }

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
) -> dynerror::Result<()> {
    println!("http listening on {}", HTTP_ADDR);
    loop {
        let (stream, _) =
            listener.accept().context("couldn't accept connection")?;
        let state_for_client = Arc::clone(&state);
        thread::spawn(move || {
            handle_http_connection(stream, state_for_client);
        });
    }
}

fn run_repo_watcher(
    repo_path: PathBuf,
    state: Arc<AppState>,
) -> dynerror::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |result| {
        let _ = tx.send(result);
    })
    .context("couldn't create filesystem watcher")?;

    watcher
        .watch(&repo_path, RecursiveMode::Recursive)
        .context("couldn't start file watcher")?;
    println!("watching repo at {}", repo_path.display());

    loop {
        match rx.recv() {
            Ok(Ok(_event)) => {
                while let Ok(Ok(_)) = rx.recv_timeout(Duration::from_millis(75))
                {
                }
                let status = state.backend.read_status(&repo_path);
                let json = status.serialize_json();
                {
                    let clients = state.clients.lock().unwrap();
                    for (id, tx) in clients.iter() {
                        match tx.send(json.clone()) {
                            Ok(()) => (),
                            Err(err) => {
                                eprintln!(
                                    "failed to send status update to client {}: {}",
                                    id,
                                    err.to_string()
                                )
                            }
                        }
                    }
                }
            }
            Ok(Err(err)) => eprintln!("watch error: {}", err),
            Err(_) => break,
        }
    }

    Ok(())
}

fn main() -> dynerror::Result<()> {
    let repo_path = parse_repo_path_from_args()?;
    let backend: Arc<dyn git::Backend> = Arc::new(git::CliBackend::new());

    let initial_snapshot = backend.read_status(&repo_path);
    if !initial_snapshot.error.is_empty() {
        bail!("couldn't load initial status: {}", initial_snapshot.error);
    }

    let state = Arc::new(AppState {
        repo_path: repo_path.clone(),
        backend,
        current_status: Mutex::new(initial_snapshot),
        clients: Mutex::new(Vec::new()),
        next_client_id: AtomicUsize::new(1),
    });

    let http_listener = TcpListener::bind(HTTP_ADDR)
        .context(format!("couldn't bind to '{}'", HTTP_ADDR))?;

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
            if let Err(e) =
                run_repo_watcher(repo_path.to_path_buf(), state_for_watcher)
            {
                eprintln!("watcher error: {}", e);
            }
        });
    }

    run_http_server(http_listener, state)
}
