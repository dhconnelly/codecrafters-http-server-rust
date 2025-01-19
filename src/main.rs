use clap::Parser;
use codecrafters_http_server::*;
use signal_hook::{consts::TERM_SIGNALS, flag, iterator::Signals};
use std::{
    fs::File,
    io::{self, BufRead},
    os::unix::fs::MetadataExt,
    sync::{atomic::AtomicBool, Arc},
    thread,
};

// TODO: use Read and move to a different module
struct SizedReader<'t> {
    r: &'t mut dyn BufRead,
    lim: usize,
    n: usize,
}

impl<'t> SizedReader<'t> {
    fn new(r: &'t mut dyn BufRead, lim: usize) -> Self {
        Self { r, lim, n: 0 }
    }
}

impl io::Read for SizedReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = if self.n < self.lim {
            let k = self.r.read(buf)?;
            self.n += k;
            k
        } else {
            0
        };
        Ok(n)
    }
}

fn codecrafters_handler() -> Box<dyn Handler> {
    Router::default()
        .route(Method::Get, "^/$", |_ctx: &Context, _req: Request| Ok(Response::empty()))
        .route(Method::Get, "^/echo/([^/]+)$", |_ctx: &Context, req: Request| {
            let message = req.matches.unwrap().swap_remove(1).unwrap();
            Ok(Response::plain_text(message))
        })
        .route(Method::Get, "^/user-agent$", |_ctx: &Context, req: Request| {
            let user_agent = req.get_header("User-Agent").ok_or(HttpStatus::BadRequest)?;
            Ok(Response::plain_text(user_agent.to_owned()))
        })
        .route(Method::Get, "^/files/([^/]+)$", |ctx: &Context, req: Request| {
            let filename = req.matches.unwrap().swap_remove(1).unwrap();
            let path = ctx.working_dir.join(filename);
            let file = File::open(path).map_err(|_| HttpStatus::NotFound)?;
            let size = file.metadata().map_err(|_| HttpStatus::NotFound)?.size();
            Ok(Response::binary(Box::new(file), size))
        })
        .route(Method::Get, "^/test-post", |_ctx: &Context, _req: Request| {
            Ok(Err(HttpStatus::BadRequest)?)
        })
        .route(Method::Post, "^/test-post", |_ctx: &Context, _req: Request| Ok(Response::empty()))
        .route(Method::Post, "^/files/([^/]+)$", |ctx: &Context, req: Request| {
            let size: usize = req
                .get_header("content-length")
                .ok_or(HttpStatus::BadRequest)?
                .parse()
                .map_err(|_| HttpStatus::BadRequest)?;
            // TODO: if route matches we can always propagate non-none |matches|
            // TODO: if route matches then we should statically know the len and avoid the get() option
            let filename = req.matches.as_ref().unwrap().get(1).unwrap().as_ref().unwrap();
            let path = ctx.working_dir.join(filename);
            let mut file = File::create_new(path).map_err(|_| HttpStatus::BadRequest)?;
            let mut from = SizedReader::new(req.body, size);
            io::copy(&mut from, &mut file).map_err(|err| {
                eprintln!("error: {}", err);
                HttpStatus::ServerError
            })?;
            Ok(Response::created())
        })
        .into()
}

fn make_server(config: Config) -> Arc<Server> {
    Arc::new(Server::start(config, codecrafters_handler()))
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
    let server = make_server(Config::parse());
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

    #[test]
    fn test_root() {
        let server = make_server(Config::default());
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}", server.addr())).unwrap();
        assert!(resp.status().is_success());
    }

    #[test]
    fn test_not_found() {
        let server = make_server(Config::default());
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}/foo", server.addr())).unwrap();
        assert!(resp.status().is_client_error());
    }

    #[test]
    fn test_echo() {
        let server = make_server(Config::default());
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let resp = reqwest::blocking::get(format!("http://{}/echo/foo", server.addr())).unwrap();
        assert!(resp.status().is_success());
        assert_eq!(resp.text().unwrap(), "foo");
    }

    #[test]
    fn test_user_agent() {
        let server = make_server(Config::default());
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let client =
            reqwest::blocking::Client::builder().user_agent("codecrafters").build().unwrap();
        let resp = client.get(format!("http://{}/user-agent", server.addr())).send().unwrap();
        assert!(resp.status().is_success());
        assert_eq!(resp.text().unwrap(), "codecrafters");
    }

    // TODO: move this test to handlers.rs
    #[test]
    fn test_post() {
        let server = make_server(Config::default());
        let server2 = Arc::clone(&server);
        thread::spawn(move || server2.listen_forever().unwrap());

        let client = reqwest::blocking::Client::new();
        let resp = client.post(format!("http://{}/test-post", server.addr())).send().unwrap();
        assert!(resp.status().is_success());
    }
}
