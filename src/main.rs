use clap::Parser;
use std::{
    io::{self, Write},
    net::{TcpListener, TcpStream},
    sync::Mutex,
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

impl Connection {
    fn new(config: &Config, stream: TcpStream) -> Result<Self, io::Error> {
        stream.set_write_timeout(Some(Duration::from_millis(config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(config.read_timeout_ms)))?;
        Ok(Self { stream })
    }

    fn handle(&mut self) {
        let s = &mut self.stream;
        if let Err(err) = write!(s, "HTTP/1.1 200 OK\r\n\r\n") {
            eprintln!("client {}: error: {}", s.peer_addr().unwrap(), err);
        }
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
                Ok(stream) => Connection::new(&self.config, stream)?.handle(),
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
