use clap::Parser;
use regex::Regex;
use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Mutex, OnceLock},
    time::Duration,
};

#[derive(Parser)]
#[command(version, about)]
struct Config {
    #[arg(default_value = "127.0.0.1")]
    host: String,
    #[arg(default_value = "4221")]
    port: u16,
    #[arg(default_value = "1000")]
    write_timeout_ms: u64,
    #[arg(default_value = "1000")]
    read_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::from("127.0.0.1"),
            port: 0,
            write_timeout_ms: 1000,
            read_timeout_ms: 1000,
        }
    }
}

struct Connection {
    stream: TcpStream,
}

#[derive(Debug)]
struct ConnectionError(String);

#[derive(Debug)]
struct RequestParsingError;

impl Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<io::Error> for ConnectionError {
    fn from(err: io::Error) -> Self {
        Self(format!("io error: {}", err))
    }
}

impl From<RequestParsingError> for ConnectionError {
    fn from(err: RequestParsingError) -> Self {
        Self(format!("failed to parse request"))
    }
}

impl Error for ConnectionError {}

struct Request<'t> {
    path: String,
    body: &'t mut dyn BufRead,
}

fn request_line_pat() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new("^GET (/[^ ]*) HTTP/1.1\r\n$").unwrap())
}

fn parse_path(reader: &mut dyn BufRead) -> Result<String, ConnectionError> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let caps = request_line_pat()
        .captures(&request_line)
        .ok_or(RequestParsingError)?;
    let path = caps.get(1).ok_or(RequestParsingError)?.as_str().to_owned();
    Ok(path)
}

fn write_status(writer: &mut dyn Write, status: HttpStatus) -> Result<(), ConnectionError> {
    Ok(write!(writer, "HTTP/1.1 {}\r\n", status)?)
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum ServerState {
    Stopped,
    Running,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpStatus {
    OK,
    BadRequest,
    NotFound,
}

impl HttpStatus {
    fn code(self) -> u16 {
        match self {
            HttpStatus::BadRequest => 400,
            HttpStatus::NotFound => 404,
            HttpStatus::OK => 200,
        }
    }
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
struct HttpError(HttpStatus);

impl Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for HttpError {}

type HttpResult = Result<Response, HttpError>;

struct Response {
    status: HttpStatus,
    body: Box<dyn Read>,
}

trait Handler: Send + Sync {
    fn handle(&self, req: Request) -> Result<Response, HttpError>;
}

struct NoopHandler;

impl From<NoopHandler> for Box<dyn Handler> {
    fn from(value: NoopHandler) -> Self {
        Box::new(value)
    }
}

impl Handler for NoopHandler {
    fn handle(&self, req: Request) -> Result<Response, HttpError> {
        let data: &[u8] = &[];
        Ok(Response { status: HttpStatus::OK, body: Box::new(data) })
    }
}

#[derive(Default)]
struct Router {
    routes: Vec<(regex::Regex, Box<dyn Handler>)>,
}

impl Router {
    fn route<H: Into<Box<dyn Handler>>>(mut self, pat: &str, handler: H) -> Self {
        self.routes.push((Regex::new(pat).unwrap(), handler.into()));
        self
    }

    fn build(self) -> Box<Self> {
        Box::new(self)
    }
}

impl Handler for Router {
    fn handle(&self, req: Request) -> Result<Response, HttpError> {
        for (pat, handler) in &self.routes {
            if pat.is_match(&req.path) {
                return handler.handle(req);
            }
        }
        Err(HttpError(HttpStatus::NotFound))
    }
}

struct Server {
    config: Config,
    addr: String,
    listener: TcpListener,
    state: Mutex<ServerState>,
    handler: Box<dyn Handler>,
}

impl Server {
    fn start(config: Config, handler: Box<dyn Handler>) -> Self {
        let addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&addr).unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state = Mutex::new(ServerState::Stopped);
        Self { config, listener, addr, state, handler }
    }

    fn stop(&self) {
        {
            let mut guard = self.state.lock().unwrap();
            if *guard != ServerState::Running {
                return;
            }
            *guard = ServerState::Stopping;
        }
        let _ = TcpStream::connect(&self.addr);
    }

    fn handle(&self, stream: io::Result<TcpStream>) -> Result<(), ConnectionError> {
        let stream = stream?;
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);

        stream.set_write_timeout(Some(Duration::from_millis(self.config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(self.config.read_timeout_ms)))?;

        let path = parse_path(&mut reader)?;
        let request = Request { path, body: &mut reader };
        let response = self.handler.handle(request);
        match response {
            Err(err) => write_status(&mut writer, err.0)?,
            Ok(resp) => write_status(&mut writer, resp.status)?,
        }

        Ok(())
    }

    fn listen(&self) -> io::Result<()> {
        {
            let mut guard = self.state.lock().unwrap();
            if *guard != ServerState::Stopped {
                return Ok(());
            }
            *guard = ServerState::Running;
        }
        for stream in self.listener.incoming() {
            if *self.state.lock().unwrap() == ServerState::Stopping {
                break;
            }
            if let Err(err) = self.handle(stream) {
                eprintln!("failed to handle connection: {}", err);
            }
        }
        {
            let mut guard = self.state.lock().unwrap();
            *guard = ServerState::Stopped;
        }
        Ok(())
    }
}

impl<F: Fn(Request) -> HttpResult + Send + Sync + 'static> Handler for F {
    fn handle(&self, req: Request) -> Result<Response, HttpError> {
        self(req)
    }
}

impl<F: Fn(Request) -> HttpResult + Send + Sync + 'static> From<F> for Box<dyn Handler> {
    fn from(value: F) -> Self {
        Box::new(value)
    }
}

fn main() {
    let config = Config::parse();
    let server = Server::start(
        config,
        Router::default()
            .route("^/$", |_req: Request<'_>| {
                let data: &[u8] = &[];
                Ok(Response { body: Box::new(data), status: HttpStatus::OK })
            })
            .build(),
    );
    println!("listening at http://{}", server.addr);
    server.listen().expect("failure");
}

#[cfg(test)]
mod test {
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_foo() {
        for _ in 0..10 {
            let config = Config::default();
            let server = Arc::new(Server::start(config, Box::new(NoopHandler)));
            let addr = server.addr.clone();

            for _ in 0..10 {
                let server2 = Arc::clone(&server);
                let handle = thread::spawn(move || server2.listen());

                for _ in 0..10 {
                    // try connecting
                    let conn = TcpStream::connect(&addr);
                    assert!(conn.is_ok());
                    // let resp = reqwest::blocking::get(&addr).unwrap();
                    // assert!(resp.status().is_success());
                    // assert_eq!(resp.text().unwrap(), "");
                }

                server.stop();
                handle.join().unwrap().expect("server failed");
            }
        }
    }
}
