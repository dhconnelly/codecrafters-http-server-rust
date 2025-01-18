use clap::Parser;
use codecrafters_http_server::*;
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

fn codecrafters_handler() -> Box<dyn Handler> {
    Router::default()
        .route("^/$", |_req: Request| Ok(Response::empty()))
        .route("^/echo/([^/]+)$", |req: Request| {
            let message = req.matches.unwrap().swap_remove(1).unwrap();
            Ok(Response::plain_text(message))
        })
        .route("^/user-agent$", |req: Request| {
            let user_agent = req.get_header("User-Agent").ok_or(HttpStatus::BadRequest)?;
            Ok(Response::plain_text(user_agent.to_owned()))
        })
        .into()
}

fn main() {
    // listen for SIGTERM, immediately quit if received twice
    let term_now = Arc::new(AtomicBool::new(false));
    for sig in TERM_SIGNALS {
        flag::register_conditional_shutdown(*sig, 1, Arc::clone(&term_now)).unwrap();
        flag::register(*sig, Arc::clone(&term_now)).unwrap();
    }
    let mut sigs = Signals::new(TERM_SIGNALS).unwrap();

    // run the server in a background thread
    let config = Config::parse();
    let handler = codecrafters_handler();
    let server = Arc::new(Server::start(config, handler));
    let server2 = Arc::clone(&server);
    let handle = thread::spawn(move || {
        server2.listen_forever().expect("failed to start server");
    });

    // wait for SIGTERM, then exit
    println!("listening at http://{}", server.addr());
    sigs.forever().next();
    println!("stopping...");
    server.stop();
    handle.join().unwrap();
    println!("server stopped, exiting");
}

#[cfg(test)]
mod test {
    use super::*;

    fn make_server() -> Arc<Server> {
        let mut config = Config::default();
        config.port = 0;
        let handler = codecrafters_handler();
        Arc::new(Server::start(config, handler))
    }

    #[test]
    fn test_root() {
        let server = make_server();
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}", server.addr())).unwrap();
        assert!(resp.status().is_success());
    }

    #[test]
    fn test_not_found() {
        let server = make_server();
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}/foo", server.addr())).unwrap();
        assert!(resp.status().is_client_error());
    }

    #[test]
    fn test_echo() {
        let server = make_server();
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}/echo/foo", server.addr())).unwrap();
        assert!(resp.status().is_success());
        assert_eq!(resp.text().unwrap(), "foo");
    }

    #[test]
    fn test_user_agent() {
        let server = make_server();
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let client =
            reqwest::blocking::Client::builder().user_agent("codecrafters").build().unwrap();
        let resp = client.get(format!("http://{}/user-agent", server.addr())).send().unwrap();
        assert!(resp.status().is_success());
        assert_eq!(resp.text().unwrap(), "codecrafters");
    }
}
