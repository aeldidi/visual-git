use std::env;
use std::error::Error;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use nanoserde::SerJson;
use notify::{RecursiveMode, Watcher};

mod git;
mod http;
mod router;
mod routes;
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
    current_status: Mutex<git::StatusSnapshot>,
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

fn serve_http(
    mut stream: TcpStream,
    state: Arc<AppState>,
) -> Result<(), Box<dyn Error>> {
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
        // TODO: propagate response
        Ok(resp) => http::write_http_response_bytes(
            &mut stream,
            "200 OK",
            "application/json",
            &resp.body.unwrap_or(vec![]),
        )?,
        // TODO: write error response
        Err(err) => http::write_http_response_bytes(
            &mut stream,
            "500 Internal Server Error",
            "text/plain; charset=utf8",
            err,
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
        current_status: Mutex::new(initial_snapshot),
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
