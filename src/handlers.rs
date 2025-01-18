use std::path::PathBuf;

use regex::Regex;

use crate::{HttpError, HttpStatus, Request, Response};

pub struct Context {
    pub working_dir: PathBuf,
}

pub trait Handler: Send + Sync {
    fn handle(&self, ctx: &Context, req: Request) -> Result<Response, HttpError>;
}

impl<H: Handler + 'static> From<H> for Box<dyn Handler> {
    fn from(value: H) -> Self {
        Box::new(value)
    }
}

impl<F> Handler for F
where
    F: Fn(&Context, Request) -> Result<Response, HttpError> + Send + Sync + 'static,
{
    fn handle(&self, ctx: &Context, req: Request) -> Result<Response, HttpError> {
        self(ctx, req)
    }
}

#[derive(Default)]
pub struct Router {
    routes: Vec<(regex::Regex, Box<dyn Handler>)>,
}

impl Router {
    pub fn route<H: Into<Box<dyn Handler>>>(mut self, pat: &str, handler: H) -> Self {
        self.routes.push((Regex::new(pat).unwrap(), handler.into()));
        self
    }
}

impl Handler for Router {
    fn handle(&self, ctx: &Context, mut req: Request) -> Result<Response, HttpError> {
        for (pat, handler) in &self.routes {
            if let Some(caps) = pat.captures(&req.path) {
                req.matches = Some(caps.iter().map(|x| x.map(|m| m.as_str().to_owned())).collect());
                return handler.handle(ctx, req);
            }
        }
        Err(HttpError(HttpStatus::NotFound))
    }
}
