use clap::Parser;
use regex::Regex;
use signal_hook::{consts::TERM_SIGNALS, flag, iterator::Signals};
use std::{
    collections::HashMap,
    error::Error,
    fmt::Display,
    io::{self, BufRead, BufReader, BufWriter, Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{atomic::AtomicBool, Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

#[derive(Parser)]
#[command(version, about)]
struct Config {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "4221")]
    port: u16,
    #[arg(long, default_value = "1000")]
    write_timeout_ms: u64,
    #[arg(long, default_value = "1000")]
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
    fn from(_err: RequestParsingError) -> Self {
        Self(format!("failed to parse request"))
    }
}

impl Error for ConnectionError {}

struct Request<'t> {
    path: String,
    matches: Option<Vec<Option<String>>>,
    headers: HashMap<String, String>,
    body: &'t mut dyn BufRead,
}

fn parse_request<'t>(reader: &'t mut dyn BufRead) -> Result<Request<'t>, ConnectionError> {
    let mut lines = reader.lines();

    // path
    static PATH: OnceLock<Regex> = OnceLock::new();
    let pat = PATH.get_or_init(|| Regex::new("^GET (/[^ ]*) HTTP/1.1$").unwrap());
    let line = lines.next().ok_or(RequestParsingError)??;
    let path = pat.captures(&line).ok_or(RequestParsingError)?[1].to_owned();

    // headers
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let pat = HEADER.get_or_init(|| Regex::new("^([^ ]+): (.+)$").unwrap());
    let mut headers = HashMap::new();
    for line in lines {
        let line = line?;
        if line.is_empty() {
            break;
        }
        let caps = pat.captures(&line).ok_or(RequestParsingError)?;
        let (key, value) = (caps[1].to_owned(), caps[2].to_owned());
        headers.insert(key, value);
    }

    Ok(Request { path, headers, body: reader, matches: None })
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
struct HttpError(HttpStatus);

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

type HttpResult = Result<Response, HttpError>;

struct Body {
    data: Box<dyn Read>,
    content_length: usize,
    content_type: String,
}

struct Response {
    status: HttpStatus,
    body: Option<Body>,
}

impl Response {
    fn empty() -> Self {
        Response { status: HttpStatus::OK, body: None }
    }

    fn plain_text(text: String) -> Self {
        let content_length = text.len();
        let content_type = String::from("text/plain");
        let data = Box::new(Cursor::new(text.into_bytes()));
        Response { status: HttpStatus::OK, body: Some(Body { content_length, content_type, data }) }
    }
}

trait Handler: Send + Sync {
    fn handle(&self, req: Request) -> Result<Response, HttpError>;
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
}

impl Handler for Router {
    fn handle(&self, mut req: Request) -> Result<Response, HttpError> {
        for (pat, handler) in &self.routes {
            if let Some(caps) = pat.captures(&req.path) {
                req.matches = Some(caps.iter().map(|x| x.map(|m| m.as_str().to_owned())).collect());
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
    fn start<H: Into<Box<dyn Handler>>>(config: Config, handler: H) -> Self {
        let addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&addr).unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state = Mutex::new(ServerState::Stopped);
        let handler = handler.into();
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

    fn addr(&self) -> &str {
        &self.addr
    }

    fn handle(&self, stream: io::Result<TcpStream>) -> Result<(), ConnectionError> {
        let stream = stream?;
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);

        stream.set_write_timeout(Some(Duration::from_millis(self.config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(self.config.read_timeout_ms)))?;

        let request = parse_request(&mut reader)?;
        let response = self.handler.handle(request);
        match response {
            Err(HttpError(status)) => {
                write!(writer, "HTTP/1.1 {}\r\n", status)?;
                write!(writer, "\r\n")?;
            }
            Ok(Response { status, mut body }) => {
                write!(writer, "HTTP/1.1 {}\r\n", status)?;
                if let Some(Body { content_length, content_type, .. }) = &body {
                    write!(writer, "Content-Type: {}\r\n", content_type)?;
                    write!(writer, "Content-Length: {}\r\n", content_length)?;
                }
                write!(writer, "\r\n")?;
                if let Some(Body { ref mut data, .. }) = &mut body {
                    io::copy(data, &mut writer)?;
                }
            }
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

impl<H: Handler + 'static> From<H> for Box<dyn Handler> {
    fn from(value: H) -> Self {
        Box::new(value)
    }
}

fn main() {
    let config = Config::parse();
    let server = Arc::new(Server::start(
        config,
        Router::default()
            .route("^/$", |_req: Request| Ok(Response::empty()))
            .route("^/echo/([^/]+)$", |req: Request| {
                let message = req.matches.unwrap().swap_remove(1).unwrap();
                Ok(Response::plain_text(message))
            })
            .route("^/user-agent$", |req: Request| {
                let user_agent = req.headers.get("User-Agent").ok_or(HttpStatus::BadRequest)?;
                Ok(Response::plain_text(user_agent.to_owned()))
            }),
    ));

    // handle double-terminate
    let term_now = Arc::new(AtomicBool::new(false));
    for sig in TERM_SIGNALS {
        flag::register_conditional_shutdown(*sig, 1, Arc::clone(&term_now)).unwrap();
        flag::register(*sig, Arc::clone(&term_now)).unwrap();
    }

    let mut sigs = Signals::new(TERM_SIGNALS).unwrap();

    let server2 = Arc::clone(&server);
    let handle = thread::spawn(move || {
        println!("listening at http://{}", server2.addr());
        server2.listen().expect("failed to start server");
    });

    for _ in &mut sigs {
        break;
    }

    println!("stopping...");
    server.stop();

    handle.join().unwrap();
    println!("server stopped, exiting");
}

#[cfg(test)]
mod test {
    use std::{sync::Arc, thread};

    use super::*;

    #[test]
    fn test_foo() {
        for _ in 0..10 {
            let config = Config::default();
            let server = Arc::new(Server::start(config, |_req: Request<'_>| {
                Ok(Response { body: None, status: HttpStatus::OK })
            }));
            let addr = format!("http://{}", server.addr);

            for _ in 0..10 {
                let server2 = Arc::clone(&server);
                let handle = thread::spawn(move || server2.listen());

                for _ in 0..10 {
                    let resp = reqwest::blocking::get(&addr).unwrap();
                    assert!(resp.status().is_success());
                    assert_eq!(resp.text().unwrap(), "");
                }

                server.stop();
                handle.join().unwrap().expect("server failed");
            }
        }
    }
}
