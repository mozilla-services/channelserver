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
extern crate cadence;
extern crate ipnet;
extern crate maxminddb;
extern crate reqwest;

use std::path::Path;
use std::time::{Duration, Instant};

use actix::Arbiter;
use actix_web::server::HttpServer;
use actix_web::{fs, http, ws, App, Error, HttpRequest, HttpResponse};
use uuid::Uuid;

mod ip_rate_limit;
mod logging;
mod meta;
mod perror;
mod server;
mod session;
mod settings;
// mod metrics;

/*
 * based on the Actix websocket example ChatServer
 */

/// Entry point for our route
fn channel_route(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // not sure if it's possible to have actix_web parse the path and have a properly
    // scoped request, since the calling structure is different for the two, so
    // manually extracting the id from the path.
    let mut path: Vec<_> = req.path().split("/").collect();
    let channel =
        Uuid::parse_str(path.pop().unwrap_or_else(|| "")).unwrap_or_else(|_| Uuid::new_v4());
    &req.state().log.do_send(logging::LogMessage {
        level: logging::ErrorLevel::Info,
        msg: format!("Creating session for channel: \"{}\"", channel.to_simple()),
    });
    let meta_info = meta::SenderData::from(req.clone());

    ws::start(
        req,
        session::WsChannelSession {
            id: 0,
            hb: Instant::now(),
            expiry: Instant::now() + Duration::from_secs(req.state().connection_lifespan),
            channel: channel,
            meta: meta_info,
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
    Ok(HttpResponse::Ok().into())
}

fn show_version(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // Return the contents of the version.json file.
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(include_str!("../version.json")))
}

fn build_app(app: App<session::WsChannelSessionState>) -> App<session::WsChannelSessionState> {
    let mut mapp = app
        // websocket to an existing channel
        .resource("/v1/ws/{channel}", |r| r.route().f(channel_route))
        // connecting to an empty channel creates a new one.
        .resource("/v1/ws/", |r| r.route().f(channel_route))
        .resource("/__version__", |r| {
            r.method(http::Method::GET).f(show_version)
        })
        .resource("/__heartbeat__", |r| {
            r.method(http::Method::GET).f(heartbeat)
        })
        .resource("/__lbheartbeat__", |r| {
            r.method(http::Method::GET).f(lbheartbeat)
        });
    // Only add a static handler if the static directory exists.
    if Path::new("static/").exists() {
        mapp = mapp.handler("/static/", fs::StaticFiles::new("static/").unwrap());
    }
    mapp
}

fn main() {
    let _ = env_logger::init();
    let sys = actix::System::new("channelserver");

    // Start chat server actor in separate thread
    let logger = logging::MozLogger::new();
    let settings = settings::Settings::new().unwrap();
    let addr = format!("{}:{}", settings.hostname, settings.port);
    let server = Arbiter::start(|_| server::ChannelServer::default());
    let log = Arbiter::start(|_| logging::MozLogger::default());
    let msettings = settings.clone();
    let mut trusted_list: Vec<ipnet::IpNet> = Vec::new();
    let ip_rep = ip_rate_limit::IPReputation::from(&settings);
    // Add the list of trusted proxies.
    if settings.trusted_proxy_list.len() > 0 {
        for mut proxy in settings.trusted_proxy_list.split(",") {
            proxy = proxy.trim();
            if proxy.len() > 0 {
                let addr: ipnet::IpNet = proxy.parse().unwrap();
                trusted_list.push(addr);
            }
        }
    }
    // check that the maxmind db is where it should be.
    if !Path::new(&settings.mmdb_loc).exists() {
        slog_error!(
            logger.log,
            "Cannot find geoip database: {}",
            settings.mmdb_loc
        );
        return;
    };
    let db_loc = settings.mmdb_loc.clone();
    let connection_lifespan = settings.conn_lifespan;
    let client_timeout = settings.client_timeout;
    // Create Http server with websocket support
    HttpServer::new(move || {
        /*
        let metrics  = metrics::metrics_from_opts(
            &msettings, logging::MozLogger::default()).unwrap();
        */
        let iploc = maxminddb::Reader::open(&db_loc).unwrap_or_else(|x| {
            use std::process::exit;
            println!("Could not read geoip database {:?}", x);
            exit(1);
        });
        // Websocket sessions state
        let state = session::WsChannelSessionState {
            addr: server.clone(),
            log: log.clone(),
            iploc,
            // metrics,
            trusted_proxy_list: trusted_list.clone(),
            ip_rep: Some(ip_rep.clone()),
            connection_lifespan,
            client_timeout,
        };

        build_app(App::with_state(state))
    })
    .bind(&addr)
    .unwrap()
    .start();

    info!(logger.log, "Started http server: {}\n{:?}", addr, settings);
    let _ = sys.run();
}

#[cfg(test)]
mod test {
    use std::str;

    use actix_web::test;
    use actix_web::ws;
    use actix_web::HttpMessage;
    use futures::Stream;
    // use cadence::{StatsdClient, NopMetricSink};

    use super::*;
    fn get_server() -> test::TestServer {
        let srv = test::TestServer::build_with_state(|| {
            let server = Arbiter::start(|_| server::ChannelServer::default());
            let log = Arbiter::start(|_| logging::MozLogger::default());
            let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
            let settings = settings::Settings::new().ok().unwrap();
            let ip_rep = ip_rate_limit::IPReputation::from(&settings);
            // let metrics = StatsdClient::builder("autopush", NopMetricSink).build();

            session::WsChannelSessionState {
                addr: server.clone(),
                log: log.clone(),
                iploc,
                // metrics,
                trusted_proxy_list: vec![],
                ip_rep: Some(ip_rep),
                connection_lifespan: 60,
                client_timeout: 30,
            }
        });
        srv.start(|app| {
            // Make this a trait eventually, for now, just copy build_app
            app.resource("/", |r| {
                r.method(http::Method::GET)
                    .f(|_| HttpResponse::NotFound().finish())
            })
            // websocket to an existing channel
            .resource("/v1/ws/{channel}", |r| r.route().f(channel_route))
            // connecting to an empty channel creates a new one.
            .resource("/v1/ws/", |r| r.route().f(channel_route))
            .resource("/__version__", |r| {
                r.method(http::Method::GET).f(show_version)
            })
            .resource("/__heartbeat__", |r| {
                r.method(http::Method::GET).f(heartbeat)
            })
            .resource("/__lbheartbeat__", |r| {
                r.method(http::Method::GET).f(lbheartbeat)
            });
        })
    }

    #[test]
    fn test_heartbeats() {
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
        /// Test broken.
        // Something in actix REALLY doesn't like having two sockets talk to
        // each other. This test will create the sockets, but messages sent
        // between them get lost somewhere interally.
        // Sometimes the messages make it through and get processed by
        // the server, however, most times they simply don't get beyond the
        // write. In any case, the recipient (reader1) never gets the
        // message and the test hangs forever.
        //
        // for now, use the ../test_chan
        let mut srv = get_server();
        let (mut reader1, writer1) = srv.ws_at("/v1/ws/").unwrap();
        let (item, r) = srv.execute(reader1.into_future()).unwrap();
        reader1 = r;
        let link_addr = read(item.unwrap());
        println!("Connecting to {:?}", link_addr);
        let (reader2, mut writer2) = srv.ws_at(&link_addr).unwrap();
        let (item, r) = srv.execute(reader2.into_future()).unwrap();
        let r2_addr = read(item.unwrap());
        println!("Connected to {:?}", r2_addr);
        assert_eq!(link_addr, r2_addr);
        let test_phrase = "This is a test";
        writer2.text("writer2");
        let (item, r) = srv.execute(reader1.into_future()).unwrap();
        assert_eq!(test_phrase, &read(item.unwrap()));
    }
}
