//! An extremely simple HTTP router.

use std::{collections::HashMap, error::Error};

use crate::http;

struct Router(HashMap<String, Route>);

pub struct Route {
    method: HashMap<
        http::Method,
        Box<dyn Fn(http::Request) -> Result<http::Response, Box<dyn Error>>>,
    >,
}

pub fn new() -> Router {
    Router(HashMap::new())
}

impl Router {
    pub fn handle(
        &self,
        path: String,
        req: http::Request,
    ) -> Result<http::Response, Box<dyn Error>> {
        match self.0.get(&path) {
            Some(route) => match route.method.get(&req.method) {
                Some(handle) => handle(req),
                None => Ok(http::method_not_allowed()),
            },
            None => Ok(http::not_found()),
        }
    }
}
