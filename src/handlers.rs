use std::{fmt::Display, path::PathBuf};

use regex::Regex;

use crate::{HttpError, HttpStatus, Method, Request, Response};

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
    routes: Vec<(Method, regex::Regex, Box<dyn Handler>)>,
}

impl Router {
    pub fn route<H: Into<Box<dyn Handler>>>(
        mut self,
        method: Method,
        pat: &str,
        handler: H,
    ) -> Self {
        self.routes.push((method, Regex::new(pat).unwrap(), handler.into()));
        self
    }
}

fn match_pat(pat: &Regex, str: &str) -> Option<Vec<Option<String>>> {
    Some(pat.captures(str)?.iter().map(|x| x.map(|m| m.as_str().to_owned())).collect())
}

impl Handler for Router {
    fn handle(&self, ctx: &Context, req: Request) -> Result<Response, HttpError> {
        let (matches, h) = self
            .routes
            .iter()
            .filter(|(method, ..)| *method == req.method)
            .filter_map(|(_, pat, h)| match_pat(pat, &req.path).map(|caps| (caps, h)))
            .next()
            .ok_or(HttpError(HttpStatus::NotFound))?;
        h.handle(ctx, req.with_matches(matches))
    }
}
