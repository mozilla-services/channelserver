#![feature(custom_derive, try_from)]
#![allow(unused_variables)]
extern crate byteorder;
extern crate bytes;
extern crate config;
extern crate env_logger;
#[macro_use]
extern crate failure;
extern crate futures;
extern crate rand;
extern crate serde;
#[macro_use]
extern crate serde_derive;
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
mod perror;
mod server;
mod session;
mod settings;

/*
 * based on the Actix websocket example ChatServer
 */

/// Entry point for our route
fn channel_route(req: HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
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
        session::WsChannelSession {
            id: 0,
            hb: Instant::now(),
            channel: channel.clone(),
            name: None,
        },
    )
}

fn main() {
    let _ = env_logger::init();
    let sys = actix::System::new("pairsona-server");
    let settings = settings::Settings::new();

    // TODO::
    // pass settings to server/logger
    // Add limiters to channel server

    // Start chat server actor in separate thread
    let logger = logging::MozLogger::new();
    let settings = settings::Settings::new().unwrap();
    let addr = format!("{}:{}", settings.hostname, settings.port);
    let server: Addr<Syn, _> = Arbiter::start(|_| server::ChannelServer::default());
    let log: Addr<Syn, _> = Arbiter::start(|_| logging::MozLogger::default());
    // Create Http server with websocket support
    HttpServer::new(move || {
        // Websocket sessions state
        let state = session::WsChannelSessionState {
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
                .resource("/v1/ws/{channel}", |r| r.route().f(channel_route))
                // connecting to an empty channel creates a new one.
                .resource("/v1/ws/", |r| r.route().f(channel_route))
                // static resources (drop?)
                .handler("/static/", fs::StaticFiles::new("static/"))
    }).bind(&addr)
        .unwrap()
        .start();

    slog_info!(logger.log, "Started http server: {}", addr);
    let _ = sys.run();
}
