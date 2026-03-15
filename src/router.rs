//! An extremely simple HTTP router.

use std::{collections::HashMap, sync::Arc};

use crate::{AppState, dynerror, http, ui_assets};

pub struct Router {
    state: Arc<AppState>,
    routes: HashMap<String, Route>,
}

type HandlerFn =
    fn(http::Request, Arc<AppState>) -> dynerror::Result<http::Response>;

pub struct Route {
    method: HashMap<http::Method, HandlerFn>,
}

pub fn new(state: Arc<AppState>) -> Router {
    Router {
        routes: HashMap::new(),
        state,
    }
}

impl Router {
    pub fn post(mut self, path: impl ToString, handler: HandlerFn) -> Self {
        let path = path.to_string();
        match self.routes.get_mut(&path) {
            Some(route) if !route.method.contains_key(&http::Method::Post) => {
                route.method.insert(http::Method::Post, handler);
            }
            None => {
                let mut route = Route {
                    method: HashMap::new(),
                };
                route.method.insert(http::Method::Post, handler);
                self.routes.insert(path, route);
            }
            _ => panic!(
                "conflicting routes: path {} has multiple POST handlers defined",
                path
            ),
        }
        self
    }
    pub fn get(mut self, path: impl ToString, handler: HandlerFn) -> Self {
        let path = path.to_string();
        match self.routes.get_mut(&path) {
            Some(route) if !route.method.contains_key(&http::Method::Get) => {
                route.method.insert(http::Method::Get, handler);
            }
            None => {
                let mut route = Route {
                    method: HashMap::new(),
                };
                route.method.insert(http::Method::Get, handler);
                self.routes.insert(path, route);
            }
            _ => panic!(
                "conflicting routes: path {} has multiple GET handlers defined",
                path
            ),
        }
        self
    }

    pub fn handle(
        &self,
        path: String,
        req: http::Request,
    ) -> dynerror::Result<http::Response> {
        match self.routes.get(&path) {
            Some(route) => match route.method.get(&req.method) {
                Some(handle) => handle(req, self.state.clone()),
                None => Ok(http::method_not_allowed()),
            },
            None => {
                let asset_path =
                    if path == "/" { "/index.html" } else { &path };
                if let Some(asset_body) = ui_assets::get(asset_path) {
                    return Ok(http::Response::builder(http::StatusCode::OK)
                        .header(
                            "Content-Type".into(),
                            content_type_for_path(asset_path).into(),
                        )
                        .body(asset_body));
                }
                Ok(http::not_found())
            }
        }
    }
}

fn content_type_for_path(path: &str) -> &'static str {
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
