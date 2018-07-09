#![allow(unused_variables)]
extern crate byteorder;
extern crate bytes;
extern crate env_logger;
extern crate futures;
extern crate rand;
extern crate serde;
extern crate serde_json;
extern crate tokio_core;
extern crate tokio_io;

#[macro_use]
extern crate actix;
extern crate actix_web;

extern crate slog;
extern crate slog_async;
extern crate uuid;
#[macro_use]
extern crate slog_term;

use std::time::Instant;

use actix::{Addr, Arbiter, Syn};
use actix_web::server::HttpServer;
use actix_web::{fs, http, ws, App, Error, HttpRequest, HttpResponse};
use uuid::Uuid;

mod logging;
mod server;
mod session;

/// Entry point for our route
fn chat_route(req: HttpRequest<session::WsChatSessionState>) -> Result<HttpResponse, Error> {
    // not sure if it's possible to have actix_web parse the path and have a properly
    // scoped request, since the calling structure is different for the two, so
    // manually extracting the id from the path.
    let preq = req.clone();
    let mut path: Vec<_> = preq.path().clone().split("/").collect();
    let channel =
        Uuid::parse_str(path.pop().unwrap_or_else(|| "")).unwrap_or_else(|_| Uuid::new_v4());
    &preq.state().log.do_send(logging::LogMessage {
        level: logging::ErrorLevel::Info,
        msg: format!("Creating session for channel: \"{}\"", channel.simple()),
    });
    ws::start(
        req,
        session::WsChatSession {
            id: 0,
            hb: Instant::now(),
            channel: channel.clone(),
            name: None,
        },
    )
}

fn main() {
    let _ = env_logger::init();
    let sys = actix::System::new("websocket-example");

    // Start chat server actor in separate thread
    let logger = logging::MozLogger::new();
    let server: Addr<Syn, _> = Arbiter::start(|_| server::ChatServer::default());
    let log: Addr<Syn, _> = Arbiter::start(|_| logging::MozLogger::default());

    // Create Http server with websocket support
    HttpServer::new(move || {
        // Websocket sessions state
        let state = session::WsChatSessionState {
            addr: server.clone(),
            log: log.clone(),
        };

        App::with_state(state)
                // redirect to websocket.html
                .resource("/", |r| r.method(http::Method::GET).f(|_| {
                    HttpResponse::NotFound()
                        .finish()
                }))
                // websocket to an existing channel
                .resource("/ws/{channel}", |r| r.route().f(chat_route))
                // connecting to an empty channel creates a new one.
                .resource("/ws/", |r| r.route().f(chat_route))
                // static resources (drop?)
                .handler("/static/", fs::StaticFiles::new("static/"))
    }).bind("127.0.0.1:8080")
        .unwrap()
        .start();

    slog_info!(logger.log, "Started http server: 127.0.0.1:8080");
    let _ = sys.run();
}
