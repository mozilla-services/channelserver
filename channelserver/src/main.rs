use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use futures::future::Future;
use serde_json::Value;
use slog::{debug, error, warn};

use actix::*;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;

#[macro_use]
mod channelid;
mod error;
mod logging;
mod meta;
mod metrics;
mod server;
mod session;
mod settings;

/* This code is modeled after the Actix example Websocket Chat Server.
   Which might explain random uses of "chat" appearing in portions of the code.
*/

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Entry point for our route
async fn channel_route(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<server::ChannelServer>>,
) -> Result<HttpResponse, Error> {
    let raw_state = req.app_data::<web::Data<session::WsChannelSessionState>>();
    let state = match raw_state {
        Some(state) => state,
        None => {
            return Ok(HttpResponse::InternalServerError().body("Invalid or missing state"));
        }
    };
    let meta = meta::SenderData::new(&req, &state);
    let mut path: Vec<&str> = req.path().split('/').collect();
    let log = logging::MozLogger::default();
    let metrics = state.metrics.clone();
    let channel = match path.pop() {
        Some(id) => {
            if id.is_empty() {
                channelid::ChannelID::default()
            } else {
                // initial_connect = false;
                match channelid::ChannelID::from_str(id) {
                    Ok(channelid) => channelid,
                    Err(err) => {
                        warn!(state.log.log, "Routing error: {:?}", err);
                        channelid::ChannelID::default()
                    }
                }
            }
        }
        None => channelid::ChannelID::default(),
    };
    ws::start(
        session::WsChannelSession {
            id: 0,
            hb: Instant::now(),
            // initial_connect: initial_connect,
            expiry: Duration::from_secs(state.settings.conn_lifespan),
            channel,
            addr: srv.get_ref().clone(),
            meta,
            log,
            metrics,
        },
        &req,
        stream,
    )
}

pub fn heartbeat(_req: HttpRequest) -> impl Future<Output = Result<HttpResponse, Error>> {
    // if there's more to check, add it here.
    let mut checklist = HashMap::new();
    checklist.insert(
        "version",
        Value::String(env!("CARGO_PKG_VERSION").to_owned()),
    );
    checklist.insert("status", Value::String("ok".to_owned()));
    HttpResponse::Ok()
        .content_type("application/json")
        .json(checklist)
}

fn lbheartbeat(_req: HttpRequest) -> impl Future<Output = Result<HttpResponse, Error>> {
    // load balance heartbeat. Doesn't matter what's returned, aside from a 200
    HttpResponse::Ok()
}

fn show_version(_req: HttpRequest) -> impl Future<Output = Result<HttpResponse, Error>> {
    // Return the contents of the version.json file.
    HttpResponse::Ok()
        .content_type("application/json")
        .body(include_str!("../version.json"))
}

pub struct Server;

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let raw_settings = settings::Settings::new();
    let settings = match raw_settings {
        Ok(settings) => settings,
        Err(e) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Bad or missing configuration {:?}", e),
            ))
        }
    };
    let addr = format!("{}:{}", settings.hostname, settings.port);
    let log = if settings.human_logs {
        logging::MozLogger::new_human()
    } else {
        logging::MozLogger::new_json()
    };

    let server = server::ChannelServer::new(&settings, &log).start();

    if !Path::new(&settings.mmdb_loc).exists() {
        error!(
            &log.log,
            "Cannot find geoip database: {}", settings.mmdb_loc
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing geoip database".to_owned(),
        ));
    };
    // Create Http server with websocket support
    debug!(&log.log, "Starting server: {:?}", &addr);
    HttpServer::new(move || {
        let state = session::WsChannelSessionState::new(&settings, &log);
        App::new()
            .data(server.clone())
            .data(state)
            .service(web::resource("/").to(|| HttpResponse::NotFound().finish()))
            // websocket
            .service(web::resource("/v1/ws/{channel}").to(channel_route))
            .service(web::resource("/v1/ws/").route(web::get().to(channel_route)))
            // static resources
            .service(web::resource("/__heartbeat__").route(web::get().to(heartbeat)))
            .service(web::resource("/__lbheartbeat__").route(web::get().to(lbheartbeat)))
            .service(web::resource("/__version__").route(web::get().to(show_version)))
    })
    .bind(addr)?
    .run()
    .await
}
