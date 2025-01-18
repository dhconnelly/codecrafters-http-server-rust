use std::{
    error::Error,
    fmt::Display,
    io::{BufRead, Cursor, Read},
};

pub struct Request<'t> {
    pub path: String,
    pub matches: Option<Vec<Option<String>>>,
    pub headers: Vec<(String, String)>,
    pub body: &'t mut dyn BufRead,
}

impl Request<'_> {
    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == key.to_lowercase())
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    OK,
    NotFound,
    BadRequest,
}

impl Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            HttpStatus::BadRequest => "400 Bad Request",
            HttpStatus::NotFound => "404 Not Found",
            HttpStatus::OK => "200 OK",
        };
        write!(f, "{}", message)
    }
}

#[derive(Debug)]
pub struct HttpError(pub HttpStatus);

impl Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<HttpStatus> for HttpError {
    fn from(value: HttpStatus) -> Self {
        Self(value)
    }
}

impl Error for HttpError {}

pub struct Body {
    pub data: Box<dyn Read>,
    pub content_length: u64,
    pub content_type: String,
}

pub struct Response {
    pub status: HttpStatus,
    pub body: Option<Body>,
}

impl Response {
    pub fn empty() -> Self {
        Response { status: HttpStatus::OK, body: None }
    }

    pub fn binary(data: Box<dyn Read>, size: u64) -> Self {
        let content_length = size;
        let content_type = "application/octet-stream".to_string();
        Response { status: HttpStatus::OK, body: Some(Body { content_length, content_type, data }) }
    }

    pub fn plain_text(text: String) -> Self {
        let content_length = text.len() as u64;
        let content_type = "text/plain".to_string();
        let data = Box::new(Cursor::new(text.into_bytes()));
        Response { status: HttpStatus::OK, body: Some(Body { content_length, content_type, data }) }
    }
}
