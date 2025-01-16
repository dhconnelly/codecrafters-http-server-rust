use clap::Parser;
use std::{
    error::Error,
    fmt::Display,
    io::{self, BufRead, BufReader, BufWriter, Read},
    net::{TcpListener, TcpStream},
    str::FromStr,
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

#[derive(PartialEq, Eq, Clone, Copy)]
enum Status {
    InvalidRequest = 400,
}

trait HttpError {
    fn status(&self) -> Status;
}

#[derive(Debug)]
struct Request {
    path: String,
}

#[derive(Debug)]
struct RequestParsingError;

impl HttpError for RequestParsingError {
    fn status(&self) -> Status {
        Status::InvalidRequest
    }
}

impl Display for RequestParsingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to parse request")
    }
}

impl Error for RequestParsingError {}

fn request_path_re() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new("^GET (/[^ ]*) HTTP/1.1$").unwrap())
}

impl FromStr for Request {
    type Err = RequestParsingError;
    fn from_str(request_line: &str) -> Result<Self, Self::Err> {
        let re = request_path_re();
        let caps = re.captures(request_line).ok_or(RequestParsingError)?;
        let path = caps.get(1).ok_or(RequestParsingError)?.as_str().to_owned();
        Ok(Request { path })
    }
}

impl Connection {
    fn new(config: &Config, stream: TcpStream) -> Result<Self, io::Error> {
        stream.set_write_timeout(Some(Duration::from_millis(config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(config.read_timeout_ms)))?;
        Ok(Self { stream })
    }

    fn handle(&mut self) {
        let mut w = BufWriter::new(&self.stream);
        let r = BufReader::new(&self.stream);
        let mut lines = BufReader::new(r).lines();

        let request_line = match lines.next() {
            Some(Ok(line)) => line,
            _ => {
                eprintln!("failed to read request line");
                return;
            }
        };

        let request = match Request::from_str(&request_line) {
            Ok(request) => request,
            _ => {
                eprintln!("failed to parse request");
                return;
            }
        };

        println!("{}: {}", self.stream.peer_addr().unwrap(), "OK");
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum ServerState {
    Stopped,
    Running,
    Stopping,
}

struct Server {
    config: Config,
    addr: String,
    listener: TcpListener,
    state: Mutex<ServerState>,
}

struct Response {
    status: Status,
    body: Box<dyn Read>,
}

trait Handler {
    fn handle(&self, req: &Request) -> Result<Response, Box<dyn HttpError>>;
}

impl Server {
    fn start(config: Config) -> Self {
        let addr = format!("{}:{}", config.host, config.port);
        let listener = TcpListener::bind(&addr).unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state = Mutex::new(ServerState::Stopped);
        Self { config, listener, addr, state }
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

    fn handle(&self, stream: TcpStream) -> io::Result<()> {
        Ok(Connection::new(&self.config, stream)?.handle())
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
            match stream {
                Ok(stream) => self.handle(stream)?,
                Err(err) => return Err(err),
            }
        }
        {
            let mut guard = self.state.lock().unwrap();
            *guard = ServerState::Stopped;
        }
        Ok(())
    }
}

fn main() {
    let config = Config::parse();
    let server = Server::start(config);
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
            let server = Arc::new(Server::start(config));
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
