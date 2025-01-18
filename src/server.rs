use crate::{thread_pool::ThreadPool, Body, Handler, HttpError, Request, Response};
use clap::Parser;
use regex::Regex;
use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, BufReader, BufWriter, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

#[derive(Debug)]
struct ConnectionError(String);

impl Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for ConnectionError {}

impl From<io::Error> for ConnectionError {
    fn from(err: io::Error) -> Self {
        Self(format!("io error: {}", err))
    }
}

#[derive(Debug)]
struct RequestParsingError;

impl From<RequestParsingError> for ConnectionError {
    fn from(_err: RequestParsingError) -> Self {
        Self("failed to parse request".to_string())
    }
}

fn parse_path(line: String) -> Result<String, ConnectionError> {
    static PATH: OnceLock<Regex> = OnceLock::new();
    let pat = PATH.get_or_init(|| Regex::new("^GET (/[^ ]*) HTTP/1.1$").unwrap());
    Ok(pat.captures(&line).ok_or(RequestParsingError)?[1].to_owned())
}

fn parse_header(line: String) -> Result<(String, String), ConnectionError> {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let pat = HEADER.get_or_init(|| Regex::new("^([^ ]+): (.+)$").unwrap());
    let caps = pat.captures(&line).ok_or(RequestParsingError)?;
    Ok((caps[1].to_owned(), caps[2].to_owned()))
}

fn parse_request(reader: &mut dyn BufRead) -> Result<Request<'_>, ConnectionError> {
    let mut lines = reader.lines();

    let path = parse_path(lines.next().ok_or(RequestParsingError)??)?;
    let headers = lines
        .take_while(|line| line.as_ref().map(|s| !s.is_empty()).unwrap_or(false))
        .map(|line| line.map_err(|err| err.into()).and_then(parse_header))
        .collect::<Result<Vec<(String, String)>, _>>()?;

    Ok(Request { path, headers, body: reader, matches: None })
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum ServerState {
    Stopped,
    Running,
    Stopping,
}

#[derive(Parser)]
#[command(version, about)]
pub struct Config {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value = "4221")]
    pub port: u16,
    #[arg(long, default_value = "1000")]
    pub write_timeout_ms: u64,
    #[arg(long, default_value = "1000")]
    pub read_timeout_ms: u64,
    #[arg(long, default_value = "4")]
    pub workers: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::from("127.0.0.1"),
            port: 0,
            write_timeout_ms: 1000,
            read_timeout_ms: 1000,
            workers: 4,
        }
    }
}

struct ConnectionHandler {
    request_handler: Box<dyn Handler>,
}

impl ConnectionHandler {
    fn new(request_handler: Box<dyn Handler>) -> Self {
        Self { request_handler }
    }

    fn handle(&self, stream: TcpStream) -> Result<(), ConnectionError> {
        let addr = stream.peer_addr().unwrap().to_string();
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);

        let request = parse_request(&mut reader)?;
        let path = request.path.clone();
        let response = self.request_handler.handle(request);
        let status = match response {
            Err(HttpError(status)) => {
                write!(writer, "HTTP/1.1 {}\r\n", status)?;
                write!(writer, "\r\n")?;
                status
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
                status
            }
        };

        println!("{}: GET {}: {}", addr, path, status);
        Ok(())
    }
}

pub struct Server {
    config: Config,
    addr: String,
    listener: TcpListener,
    state: Mutex<ServerState>,
    handler: Arc<ConnectionHandler>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Server {
    pub fn start<H: Into<Box<dyn Handler>>>(config: Config, handler: H) -> Self {
        let addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&addr).unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state = Mutex::new(ServerState::Stopped);
        let handler = Arc::new(ConnectionHandler::new(handler.into()));
        Self { config, listener, addr, state, handler }
    }

    pub fn stop(&self) {
        let mut guard = self.state.lock().unwrap();
        if *guard != ServerState::Running {
            return;
        }
        *guard = ServerState::Stopping;
        let _ = TcpStream::connect(&self.addr);
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn listen_forever(&self) -> io::Result<()> {
        // don't start if we're already running
        {
            let mut guard = self.state.lock().unwrap();
            if *guard != ServerState::Stopped {
                return Ok(());
            }
            *guard = ServerState::Running;
        }

        // run until stopped
        let mut pool = ThreadPool::new(self.config.workers);
        for stream in self.listener.incoming() {
            if *self.state.lock().unwrap() == ServerState::Stopping {
                break;
            }
            let stream = stream?;
            stream.set_write_timeout(Some(Duration::from_millis(self.config.write_timeout_ms)))?;
            stream.set_read_timeout(Some(Duration::from_millis(self.config.read_timeout_ms)))?;
            let handler = Arc::clone(&self.handler);
            pool.execute(Box::new(move || {
                if let Err(err) = handler.handle(stream) {
                    eprintln!("failed to handle connection: {}", err);
                }
            }));
        }

        // mark as stopped
        {
            let mut guard = self.state.lock().unwrap();
            *guard = ServerState::Stopped;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::HttpStatus;
    use std::{sync::Arc, thread};

    // TODO: test out of order lifecycle calls

    #[test]
    fn test_lifecycle() {
        for _ in 0..10 {
            let config = Config::default();
            let server = Arc::new(Server::start(config, |_req: Request<'_>| {
                Ok(Response { body: None, status: HttpStatus::OK })
            }));
            let addr = format!("http://{}", server.addr());

            for _ in 0..10 {
                let server2 = Arc::clone(&server);
                let handle = thread::spawn(move || server2.listen_forever());

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
