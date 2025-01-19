use crate::{
    parse_request, thread_pool::ThreadPool, Body, Context, Handler, HttpError, Request,
    RequestParsingError, Response,
};
use clap::Parser;
use regex::Regex;
use std::{
    env,
    error::Error,
    fmt::Display,
    io::{self, BufRead, BufReader, BufWriter, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
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

impl From<RequestParsingError> for ConnectionError {
    fn from(_err: RequestParsingError) -> Self {
        Self("failed to parse request".to_string())
    }
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
    #[arg(long, default_value = ".")]
    pub directory: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: String::from("127.0.0.1"),
            port: 0,
            write_timeout_ms: 1000,
            read_timeout_ms: 1000,
            workers: 4,
            directory: env::current_dir().unwrap(),
        }
    }
}

struct ConnectionHandler {
    context: Context,
    request_handler: Box<dyn Handler>,
}

impl ConnectionHandler {
    fn new(context: Context, request_handler: Box<dyn Handler>) -> Self {
        Self { context, request_handler }
    }

    fn handle(&self, stream: TcpStream) -> Result<(), ConnectionError> {
        let addr = stream.peer_addr().unwrap().to_string();
        let mut reader = BufReader::new(&stream);
        let mut writer = BufWriter::new(&stream);

        let request = parse_request(&mut reader)?;
        let path = request.path.clone();
        let response = self.request_handler.handle(&self.context, request);
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
        let context = Context { working_dir: config.directory.clone() };
        let handler = Arc::new(ConnectionHandler::new(context, handler.into()));
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
            let server = Arc::new(Server::start(config, |_ctx: &Context, _req: Request<'_>| {
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
