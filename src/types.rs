use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, Cursor, Read},
    str::FromStr,
    sync::OnceLock,
};

use regex::Regex;

#[derive(Debug)]
pub struct RequestParsingError;

impl From<io::Error> for RequestParsingError {
    fn from(_value: io::Error) -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl FromStr for Method {
    type Err = RequestParsingError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "POST" => Ok(Self::Post),
            "GET" => Ok(Self::Get),
            _ => Err(RequestParsingError),
        }
    }
}

pub struct Request<'t> {
    pub method: Method,
    pub path: String,
    pub matches: Option<Vec<Option<String>>>,
    pub headers: Vec<(String, String)>,
    pub body: &'t mut dyn BufRead,
}

impl Request<'_> {
    pub fn with_matches(mut self, matches: Vec<Option<String>>) -> Self {
        self.matches = Some(matches);
        self
    }

    pub fn get_header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == key.to_lowercase())
            .map(|(_, v)| v.as_str())
    }
}

fn parse_request_line(line: String) -> Result<(Method, String), RequestParsingError> {
    static PATH: OnceLock<Regex> = OnceLock::new();
    let pat = PATH.get_or_init(|| Regex::new("^(GET|POST) (/[^ ]*) HTTP/1.1$").unwrap());
    let caps = pat.captures(&line).ok_or(RequestParsingError)?;
    let method = caps[1].parse()?;
    let path = caps[2].to_string();
    Ok((method, path))
}

fn parse_header(line: String) -> Result<(String, String), RequestParsingError> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let pat = HEADER.get_or_init(|| Regex::new("^([^ ]+): (.+)$").unwrap());
    let caps = pat.captures(&line).ok_or(RequestParsingError)?;
    Ok((caps[1].to_owned(), caps[2].to_owned()))
}

pub fn parse_request(reader: &mut dyn BufRead) -> Result<Request<'_>, RequestParsingError> {
    let mut lines = reader.lines();
    let (method, path) = parse_request_line(lines.next().ok_or(RequestParsingError)??)?;
    let headers = lines
        .take_while(|line| line.as_ref().map(|s| !s.is_empty()).unwrap_or(false))
        .map(|line| line.map_err(|err| err.into()).and_then(parse_header))
        .collect::<Result<Vec<(String, String)>, _>>()?;
    Ok(Request { method, path, headers, body: reader, matches: None })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    OK,
    Created,
    NotFound,
    BadRequest,
    ServerError,
}

impl Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            HttpStatus::BadRequest => "400 Bad Request",
            HttpStatus::NotFound => "404 Not Found",
            HttpStatus::OK => "200 OK",
            HttpStatus::Created => "201 Created",
            HttpStatus::ServerError => "500 Internal Server Error",
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

    pub fn created() -> Self {
        Response { status: HttpStatus::Created, body: None }
    }

    pub fn plain_text(text: String) -> Self {
        let content_length = text.len() as u64;
        let content_type = "text/plain".to_string();
        let data = Box::new(Cursor::new(text.into_bytes()));
        Response { status: HttpStatus::OK, body: Some(Body { content_length, content_type, data }) }
    }
}
