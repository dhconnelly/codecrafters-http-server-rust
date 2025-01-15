use clap::Parser;
use std::{
    io,
    io::Write,
    net::{TcpListener, TcpStream},
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

struct Server {
    config: Config,
}

struct Connection {
    stream: TcpStream,
}

impl Connection {
    fn new(config: &Config, stream: TcpStream) -> Result<Self, io::Error> {
        stream.set_write_timeout(Some(Duration::from_millis(config.write_timeout_ms)))?;
        stream.set_read_timeout(Some(Duration::from_millis(config.read_timeout_ms)))?;
        return Ok(Self { stream });
    }

    fn handle(&mut self) {
        let s = &mut self.stream;
        if let Err(err) = write!(s, "HTTP/1.1 200 OK\r\n\r\n") {
            eprintln!("client {}: error: {}", s.peer_addr().unwrap(), err);
        }
    }
}

impl Server {
    fn new(config: Config) -> Self {
        Self { config }
    }

    fn handle(&self, stream: TcpStream) -> io::Result<()> {
        Ok(Connection::new(&self.config, stream)?.handle())
    }

    fn run(&mut self) -> Result<(), io::Error> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).unwrap();
        println!("listening at http://{}", addr);
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => self.handle(stream)?,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

fn main() {
    let cli = Config::parse();
    let mut server = Server::new(cli);
    server.run().expect("server failed");
}
