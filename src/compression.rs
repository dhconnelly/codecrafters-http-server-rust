use std::{
    collections::HashSet,
    error::Error,
    fmt::Display,
    io::{Cursor, Read},
};

use flate2::read::GzEncoder;

use crate::{Middleware, MiddlewareFactory, Request};

#[derive(Debug)]
pub struct DecompressionError;

impl Display for DecompressionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to decompress")
    }
}

impl Error for DecompressionError {}

pub struct CompressionFactory;

impl MiddlewareFactory for CompressionFactory {
    fn new(&self, req: &Request) -> Option<Box<dyn Middleware>> {
        let schemes: HashSet<&str> = req.get_header("accept-encoding")?.split(", ").collect();
        if schemes.contains("gzip") {
            println!("enabling gzip");
            Some(Box::new(Compression))
        } else {
            None
        }
    }
}

pub struct Compression;

impl Middleware for Compression {
    fn apply_before(&self, _req: &mut Request) -> Result<(), crate::MiddlewareError> {
        // do nothing
        Ok(())
    }

    fn apply_after(&self, resp: &mut crate::Response) -> Result<(), crate::MiddlewareError> {
        resp.set_header("content-encoding".to_string(), "gzip".to_string());
        if let Some(data) = resp.body.take() {
            let mut e = GzEncoder::new(data, flate2::Compression::fast());
            let mut buf = Vec::new();
            let size = e.read_to_end(&mut buf)?;
            resp.set_header("content-length".to_string(), size.to_string());
            resp.body = Some(Box::new(Cursor::new(buf)));
        }
        Ok(())
    }
}
