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

// TODO: move R into Box<R>
#[derive(Debug)]
struct Request<R> {
    path: String,
    body: BufReader<R>,
}

impl<R: Read> Request<R> {
    fn pat() -> &'static regex::Regex {
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        RE.get_or_init(|| regex::Regex::new("^GET (/[^ ]*) HTTP/1.1\r\n$").unwrap())
    }

    fn parse(mut reader: BufReader<R>) -> Result<Self, ConnectionError> {
        let mut line = String::new();

        // path
        reader.read_line(&mut line)?;
        let caps = Self::pat().captures(&line).ok_or(RequestParsingError)?;
        let path = caps.get(1).ok_or(RequestParsingError)?.as_str().to_owned();

        Ok(Self { path, body: reader })
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

enum HttpStatus {
    OK = 200,
    BadRequest = 400,
    NotFound = 404,
}

trait HttpError: Error {
    fn status(&self) -> HttpStatus;
}

struct Response<R: Read> {
    status: HttpStatus,
    body: R,
}

trait Handler {
    fn handle<In: Read, Out: Read>(req: &Request<In>) -> Result<Response<Out>, Box<dyn HttpError>>;
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

    fn handle(&self, stream: io::Result<TcpStream>) -> Result<(), ConnectionError> {
        let stream = stream?;

        stream.set_write_timeout(Some(Duration::from_millis(self.config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(self.config.read_timeout_ms)))?;

        let request = Request::parse(BufReader::new(&stream))?;
        println!(
            "{}: {}: {}",
            stream.peer_addr().unwrap(),
            request.path,
            "OK"
        );

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
