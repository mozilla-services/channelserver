//#![feature(custom_derive, try_from)]
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
#[macro_use]
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

use std::path::Path;
use std::time::Instant;
//use std::sync::{Arc, Mutex};
//use std::collections::HashMap;

use actix::Arbiter;
//use actix::prelude::{Recipient};
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
fn channel_route(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
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

fn heartbeat(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // if there's more to check, add it here.
    let body = json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")});
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(body.to_string()))
}

fn lbheartbeat(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // load balance heartbeat. Doesn't matter what's returned, aside from a 200
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body("{}"))
}

fn show_version(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // Return the contents of the version.json file.
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(include_str!("../version.json")))
}

fn build_app(app: App<session::WsChannelSessionState>) -> App<session::WsChannelSessionState> {
    let mut mapp = app
            // redirect to websocket.html
            .resource("/", |r| r.method(http::Method::GET).f(|_| {
                HttpResponse::NotFound()
                    .finish()
            }))
            // websocket to an existing channel
            .resource("/v1/ws/{channel}", |r| r.route().f(channel_route))
            // connecting to an empty channel creates a new one.
            .resource("/v1/ws/", |r| r.route().f(channel_route))
            .resource("/__version__", |r| r.method(http::Method::GET).f(show_version))
            .resource("/__heartbeat__", |r| r.method(http::Method::GET).f(heartbeat))
            .resource("/__lbheartbeat__", |r| r.method(http::Method::GET).f(lbheartbeat));
    // Only add a static handler if the static directory exists.
    if Path::new("static/").exists() {
        mapp = mapp.handler("/static/", fs::StaticFiles::new("static/").unwrap());
    }
    mapp
}

fn main() {
    let _ = env_logger::init();
    let sys = actix::System::new("pairsona-server");

    // Start chat server actor in separate thread
    let logger = logging::MozLogger::new();
    let settings = settings::Settings::new().unwrap();
    let addr = format!("{}:{}", settings.hostname, settings.port);
    /*
    let sessions =
        Arc::new(Mutex::new(HashMap::<usize, Recipient<server::TextMessage>>::new()));
    let channels =
        Arc::new(Mutex::new(HashMap::<Uuid, HashMap<usize, server::Channel>>::new()));
    let server = Arbiter::start(|_| server::ChannelServer::init(channels, sessions));
    */
    let server = Arbiter::start(|_| server::ChannelServer::default());
    let log = Arbiter::start(|_| logging::MozLogger::default());

    // Create Http server with websocket support
    HttpServer::new(move || {
        // Websocket sessions state
        let state = session::WsChannelSessionState {
            addr: server.clone(),
            log: log.clone(),
        };

        build_app(App::with_state(state))
    }).bind(&addr)
    .unwrap()
    .start();

    slog_info!(logger.log, "Started http server: {}", addr);
    let _ = sys.run();
}

#[cfg(test)]
mod test {
    use std::str;

    use actix_web::test;
    use actix_web::ws::{self, WsWriter};
    use actix_web::HttpMessage;
    use futures::Stream;

    use super::*;
    // use server::{ChannelCollection, SessionCollection};

    //fn get_server(channels: ChannelCollection, sessions: SessionCollection) -> test::TestServer {
    fn get_server() -> test::TestServer {
        test::TestServer::with_factory(|| {
            //let server =Arbiter::start( |_| server::ChannelServer::init(channels, sessions));
            let server = Arbiter::start(|_| server::ChannelServer::default());
            let log = Arbiter::start(|_| logging::MozLogger::default());

            let state = session::WsChannelSessionState {
                addr: server.clone(),
                log: log.clone(),
            };
            build_app(App::with_state(state))
        })
    }

    #[test]
    fn test_heartbeats() {
        /*
        let channels =
            Arc::new(Mutex::new(HashMap::<Uuid, HashMap<usize, server::Channel>>::new()));
        let sessions =
            Arc::new(Mutex::new(HashMap::<usize, Recipient<server::TextMessage>>::new()));
        let mut srv = get_server(channels.clone(), sessions.clone());
        */
        let mut srv = get_server();
        // Test the DockerFlow URLs
        {
            let request = srv.get().uri(srv.url("/__heartbeat__")).finish().unwrap();
            let response = srv.execute(request.send()).unwrap();
            assert!(response.status().is_success());
            let bytes = srv.execute(response.body()).unwrap();
            let body = str::from_utf8(&bytes).unwrap();
            assert_eq!(
                json!({"status": "ok", "version": env!("CARGO_PKG_VERSION")}).to_string(),
                body
            );
        }
        {
            let request = srv.get().uri(srv.url("/__lbheartbeat__")).finish().unwrap();
            let response = srv.execute(request.send()).unwrap();
            assert!(response.status().is_success());
            let bytes = srv.execute(response.body()).unwrap();
            let body = str::from_utf8(&bytes).unwrap();
            assert_eq!("{}", body);
        }
        {
            let request = srv.get().uri(srv.url("/__version__")).finish().unwrap();
            let response = srv.execute(request.send()).unwrap();
            assert!(response.status().is_success());
            let bytes = srv.execute(response.body()).unwrap();
            let body = str::from_utf8(&bytes).unwrap();
            assert_eq!(include_str!("../version.json"), body);
        }
    }

    fn read(msg: ws::Message) -> String {
        match msg {
            ws::Message::Text(text) => text.as_str().to_owned(),
            _ => format!("Unexpected data type {:?}", msg),
        }
    }

    #[ignore]
    #[test]
    fn test_websockets() {
        /*        let channels =
            Arc::new(Mutex::new(HashMap::<Uuid, HashMap<usize, server::Channel>>::new()));
        let sessions =
            Arc::new(Mutex::new(HashMap::<usize, Recipient<server::TextMessage>>::new()));
        let mut srv = get_server(channels.clone(), sessions.clone());
*/
        let mut srv = get_server();
        let (mut reader1, mut writer1) = srv.ws_at("/v1/ws/").unwrap();
        let (item, r) = srv.execute(reader1.into_future()).unwrap();
        reader1 = r;
        let link_addr = read(item.unwrap());
        // for unknown reasons, this appears to not open the same
        // server instance, or at least, does not open an instance with
        // the same server index table. Two new "channel" entries
        // are created.
        let (mut reader2, mut writer2) = srv.ws_at(&link_addr).unwrap();
        let (item, r) = srv.execute(reader2.into_future()).unwrap();
        let r2_addr = read(item.unwrap());
        reader2 = r;
        assert_eq!(link_addr, r2_addr);
        let test_phrase = "This is a test";
        writer1.send_text("writer1");
        writer2.text("writer2");
        let (item, r) = srv.execute(reader2.into_future()).unwrap();
        reader2 = r;
        assert_eq!(test_phrase, &read(item.unwrap()));
    }
}
