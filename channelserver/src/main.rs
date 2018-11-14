//#![feature(custom_derive, try_from)]
#![allow(unused_variables)]
extern crate base64;
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
extern crate slog_mozlog_json;
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

mod channelid;
mod logging;
mod meta;
mod metrics;
mod perror;
mod server;
mod session;
mod settings;

/*
 * based on the Actix websocket example ChatServer
 */
fn channel_route_base(
    req: &HttpRequest<session::WsChannelSessionState>,
) -> Result<HttpResponse, Error> {
    info!(&req.state().log.log, "### Channel Route Base!");
    channel_route(req)
}

/// Entry point for our route
fn channel_route(req: &HttpRequest<session::WsChannelSessionState>) -> Result<HttpResponse, Error> {
    // not sure if it's possible to have actix_web parse the path and have a properly
    // scoped request, since the calling structure is different for the two, so
    // manually extracting the id from the path.
    let mut path: Vec<_> = req.path().split("/").collect();
    let meta_info = meta::SenderData::from(req.clone());
    let mut initial_connect = true;
    let channel = match path.pop() {
        Some(id) => {
            // if the id is valid, but not present, treat it like a "None"
            if id.len() == 0 {
                channelid::ChannelID::default()
            } else {
                initial_connect = false;
                let channel_id = match channelid::ChannelID::from_str(id) {
                    Ok(channelid) => channelid,
                    Err(err) => {
                        warn!(&req.state().log.log,
                            "Invalid ChannelID specified: {:?}", id;
                            "remote_ip" => &meta_info.remote);
                        return Ok(HttpResponse::new(http::StatusCode::NOT_FOUND));
                    }
                };
                channel_id
            }
        }
        None => channelid::ChannelID::default(),
    };
    info!(
        &req.state().log.log,
        "Creating session for {} channel: \"{}\"",
        if initial_connect {"new"} else {"candiate"},
        channel.to_string();
        "remote_ip" => &meta_info.remote
    );

    ws::start(
        req,
        session::WsChannelSession {
            id: 0,
            hb: Instant::now(),
            expiry: Instant::now() + Duration::from_secs(req.state().connection_lifespan),
            channel: channel,
            meta: meta_info,
            initial_connect,
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
        // connecting to an empty channel creates a new one.
        .resource("/v1/ws/", |r| r.route().f(channel_route_base))
        // websocket to an existing channel
        .resource("/v1/ws/{channel}", |r| r.route().f(channel_route))
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
    let log = if settings.human_logs {
        logging::MozLogger::new_human()
    } else {
        logging::MozLogger::new_json()
    };
    let msettings = settings.clone();
    let mut trusted_list: Vec<ipnet::IpNet> = Vec::new();
    // Add the known private networks to the trusted proxy list
    trusted_list.push("10.0.0.0/8".parse().unwrap());
    trusted_list.push("172.16.0.0/12".parse().unwrap());
    trusted_list.push("192.168.0.0/16".parse().unwrap());

    // Add the list of trusted proxies.
    if settings.trusted_proxy_list.len() > 0 {
        for mut proxy in settings.trusted_proxy_list.split(",") {
            proxy = proxy.trim();
            if proxy.len() > 0 {
                // ipnet::IpNet only wants CIDRs. Normally that's not a problem, but the
                // user may specify a single address. In that case, force the single
                // into a CIDR by giving it a single address scope.
                let mut fixed = proxy.to_owned();
                if !proxy.contains("/") {
                    fixed = format!("{}/32", proxy);
                    debug!(log.log, "Fixing single address {}", fixed);
                }
                match fixed.parse::<ipnet::IpNet>() {
                    Ok(addr) => trusted_list.push(addr),
                    Err(err) => {
                        error!(logger.log, r#"Ignoring unparsable IP address "{}"#, proxy);
                    }
                };
            }
        }
    }
    debug!(logger.log, "Trusted Proxies: {:?}", trusted_list);
    // check that the maxmind db is where it should be.
    if !Path::new(&settings.mmdb_loc).exists() {
        error!(
            logger.log,
            "Cannot find geoip database: {}", settings.mmdb_loc
        );
        return;
    };
    let db_loc = settings.mmdb_loc.clone();
    let connection_lifespan = settings.conn_lifespan;
    let client_timeout = settings.client_timeout;
    let ping_interval = settings.heartbeat;
    // Create Http server with websocket support
    HttpServer::new(move || {
        let logging = if msettings.human_logs {
            logging::MozLogger::new_human()
        } else {
            logging::MozLogger::new_json()
        };
        let metrics = metrics::metrics_from_opts(&msettings, logging).unwrap();
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
            metrics,
            trusted_proxy_list: trusted_list.clone(),
            connection_lifespan,
            client_timeout,
            ping_interval,
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
    use actix_web::HttpMessage;
    use cadence::{NopMetricSink, StatsdClient};

    use super::*;
    fn get_server() -> test::TestServer {
        let srv = test::TestServer::build_with_state(|| {
            let server = Arbiter::start(|_| server::ChannelServer::default());
            let settings = settings::Settings::new().ok().unwrap();
            let log = if settings.human_logs {
                logging::MozLogger::new_human()
            } else {
                logging::MozLogger::default()
            };
            let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
            let metrics = StatsdClient::builder("autopush", NopMetricSink).build();

            session::WsChannelSessionState {
                addr: server.clone(),
                log,
                iploc,
                metrics,
                trusted_proxy_list: vec![],
                connection_lifespan: 60,
                client_timeout: 30,
                ping_interval: 5,
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

    // To test websocket interface, please use external test_chan
}
