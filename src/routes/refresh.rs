use std::{error::Error, sync::Arc};

use nanoserde::SerJson;

use crate::{AppState, http};

pub fn refresh_status(
    _req: http::Request,
    state: Arc<AppState>,
) -> Result<http::Response, Box<dyn Error>> {
    let status = state.backend.read_status(&state.repo_path);
    {
        let mut current = state.current_status.lock().unwrap();
        if *current != status {
            *current = status.clone();
        }
    }

    {
        let mut clients = state.clients.lock().unwrap();
        let json = status.serialize_json();
        clients.retain(|(_, tx)| tx.send(json.clone()).is_ok());
    }
    Ok(http::json(status))
}
