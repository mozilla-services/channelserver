use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cadence::CountedExt;
use serde_json::Value;
use slog::{error, info, warn};

use actix::{Actor, Addr};
use actix_web::{App, Error, HttpRequest, HttpResponse, HttpServer, http::header, web};
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

/// Entry point for our route
const RESUME_PROTOCOL_PREFIX: &str = "resume.";

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
    let meta = meta::SenderData::new(&req, state);
    let mut path: Vec<&str> = req.path().split('/').collect();
    let log = logging::MozLogger::default();
    let metrics = state.metrics.clone();
    let mut initial_connection: bool = true;
    let channel = match path.pop() {
        Some(id) => {
            if id.is_empty() {
                metrics
                    .incr_with_tags("conn.request")
                    .with_tag_value("new")
                    .send();
                channelid::ChannelID::default()
            } else {
                match channelid::ChannelID::from_str(id) {
                    Ok(channelid) => {
                        initial_connection = false;
                        metrics
                            .incr_with_tags("conn.request")
                            .with_tag_value("existing")
                            .send();
                        channelid
                    }
                    Err(err) => {
                        warn!(state.log.log, "Routing error: {:?}", err);
                        metrics
                            .incr_with_tags("conn.request")
                            .with_tag_value("error")
                            .send();
                        channelid::ChannelID::default()
                    }
                }
            }
        }

        None => {
            metrics
                .incr_with_tags("conn.request")
                .with_tag_value("none")
                .send();
            channelid::ChannelID::default()
        }
    };
    // The resume token rides in Sec-WebSocket-Protocol as `resume.<32 hex>`,
    // not the URL, which proxies and load balancers tend to log.
    let resume_protocol = req
        .headers()
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',').map(str::trim).find(|p| {
                p.strip_prefix(RESUME_PROTOCOL_PREFIX)
                    .is_some_and(|t| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()))
            })
        })
        .map(str::to_owned);
    let resume = resume_protocol
        .as_deref()
        .map(|p| p[RESUME_PROTOCOL_PREFIX.len()..].to_owned());
    // Only a client that asks for resume is held after a drop, so legacy
    // clients keep the old behavior. Older servers ignore the flag.
    let resumable = req.query_string().split('&').any(|kv| kv == "resume=1");
    let session = session::WsChannelSession {
        id: 0,
        hb: Instant::now(),
        expiry: Duration::from_secs(state.settings.conn_lifespan),
        channel,
        addr: srv.get_ref().clone(),
        initial_connection,
        resume,
        resumable,
        meta,
        log,
        metrics,
        settings: state.settings.clone(),
    };
    // A browser drops the connection unless the server echoes the protocol it asked for.
    let protocols: Vec<&str> = resume_protocol.as_deref().into_iter().collect();
    ws::WsResponseBuilder::new(session, &req, stream)
        .protocols(&protocols)
        .start()
}

/// Debug only: drop the newest participant of a channel to exercise resume.
async fn debug_drop(req: HttpRequest, srv: web::Data<Addr<server::ChannelServer>>) -> HttpResponse {
    let id = req.match_info().get("channel").unwrap_or_default();
    match channelid::ChannelID::from_str(id) {
        Ok(channel) => match srv.send(server::DropNewest(channel)).await {
            Ok(true) => HttpResponse::Ok().body("dropped"),
            _ => HttpResponse::NotFound().finish(),
        },
        Err(_) => HttpResponse::BadRequest().finish(),
    }
}

pub async fn heartbeat(_req: HttpRequest) -> HttpResponse {
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

pub async fn lbheartbeat(_req: HttpRequest) -> HttpResponse {
    // load balance heartbeat. Doesn't matter what's returned, aside from a 200
    HttpResponse::Ok().finish()
}

pub async fn show_version(_req: HttpRequest) -> HttpResponse {
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
                format!("Bad or missing configuration {e:?}"),
            ));
        }
    };

    let addr = format!("{}:{}", settings.hostname, settings.port);
    let log = if settings.human_logs {
        logging::MozLogger::new_human()
    } else {
        logging::MozLogger::new_json()
    };

    let metrics =
        Arc::new(metrics::metrics_from_opts(&settings, &log).expect("Could not create metrics"));
    let server = server::ChannelServer::new(&settings, &log, metrics.clone()).start();

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
    info!(&log.log, "Starting server: {:?}", &addr);
    let debug = settings.debug;
    HttpServer::new(move || {
        let state = session::WsChannelSessionState::new(&settings, &log, &metrics);
        App::new()
            .app_data(web::Data::new(server.clone()))
            .app_data(web::Data::new(state))
            .service(web::resource("/").route(web::get().to(HttpResponse::NotFound)))
            // websocket
            .service(web::resource("/v1/ws/{channel}").to(channel_route))
            .service(web::resource("/v1/ws/").route(web::get().to(channel_route)))
            // static resources
            .service(web::resource("/__heartbeat__").route(web::get().to(heartbeat)))
            .service(web::resource("/__lbheartbeat__").route(web::get().to(lbheartbeat)))
            .service(web::resource("/__version__").route(web::get().to(show_version)))
            .configure(|cfg| {
                if debug {
                    cfg.service(
                        web::resource("/__drop__/{channel}").route(web::get().to(debug_drop)),
                    );
                }
            })
    })
    .bind(addr)?
    .run()
    .await
}
