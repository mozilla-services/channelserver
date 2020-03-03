use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::path::Path;

use cadence::StatsdClient;
use futures::future::{Future};
use serde_json::Value;
use slog::{debug, error, info, warn};
use slog_scope;
use ipnet::IpNet;

use actix::*;
use actix_rt::System;
use actix_files as fs;
use actix_web::{web, App, Error, HttpRequest, HttpResponse, HttpServer};
use actix_web_actors::ws;

#[macro_use]
mod channelid;
mod error;
mod server;
mod settings;
mod logging;
mod metrics;
mod meta;

/*
    TODO:
    * remove `.wrap()`s and other shortcuts
    * fix logging and metric calls
    * verify integration test (test_chan)
    * remove detrius code and elements
    * fmt / fix docs.
*/


/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Entry point for our route
async fn chat_route(
    req: HttpRequest,
    stream: web::Payload,
    srv: web::Data<Addr<server::ChannelServer>>,
    meta: meta::SenderData,
) -> Result<HttpResponse, Error> {
    let state = req.app_data::<WsChannelSessionState>().unwrap();
    let mut path:Vec<&str> = req.path().split('/').collect();
    let log = logging::MozLogger::default();
    let metrics = state.metrics.clone();
    let mut initial_connect = true;
    let channel = match path.pop() {
        Some(id) => {
            if id.is_empty() {
                channelid::ChannelID::default()
            } else {
                initial_connect = false;
                match channelid::ChannelID::from_str(id) {
                    Ok(channelid) => channelid,
                    Err(err) => {
                        warn!(state.log.log, "Routing error: {:?}", err);
                        channelid::ChannelID::default()
                    }
                }
            }
        },
        None => channelid::ChannelID::default()
    };
    ws::start(
        WsChannelSession {
            id: 0,
            hb: Instant::now(),
            initial_connect: initial_connect,
            expiry: Duration::from_secs(state.connection_lifespan),
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
        "version", Value::String(env!("CARGO_PKG_VERSION").to_owned())
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

pub struct WsChannelSessionState {
    pub addr: Addr<server::ChannelServer>,
    pub log: logging::MozLogger,
    pub iploc: maxminddb::Reader<Vec<u8>>,
    pub metrics: StatsdClient,
    pub trusted_proxy_list: Vec<IpNet>,
    pub connection_lifespan: u64,
    pub client_timeout: u64,
    pub ping_interval: u64,
}


struct WsChannelSession {
    /// unique session id
    id: usize,
    /// Client must send ping at least once per 10 seconds (CLIENT_TIMEOUT),
    /// otherwise we drop connection.
    hb: Instant,
    // max channel lifespan
    expiry: Duration,
    /// joined channel
    channel: channelid::ChannelID,
    /// peer name
    meta: meta::SenderData,
    /// Chat server
    addr: Addr<server::ChannelServer>,
    // first time connecting on this channel?
    initial_connect: bool,

    // logging pointer
    log: logging::MozLogger,
    metrics: cadence::StatsdClient,
}

impl Actor for WsChannelSession {
    type Context = ws::WebsocketContext<Self>;

    /// Method is called on actor start.
    /// We register ws session with ChatServer
    fn started(&mut self, ctx: &mut Self::Context) {
        // we'll start heartbeat process on session start.
        self.hb(ctx);

        let meta = self.meta.clone();

        // register self in chat server. `AsyncContext::wait` register
        // future within context, but context waits until this future resolves
        // before processing any other events.
        // HttpContext::state() is instance of WsChannelSessionState, state is shared
        // across all routes within application
        let addr = ctx.address();
        self.addr
            .send(server::Connect {
                addr: addr.recipient(),
                initial_connect: true,
                remote: meta.remote,
            })
            .into_actor(self)
            .then(|res, act, ctx| {
                let remote = &act.meta.remote;
                match res {
                    Ok(session_id) => {
                        if session_id == 0 {
                            ctx.stop()
                        }
                        debug!(
                            act.log.log,
                            "Starting new session";
                            "session" => session_id,
                            "remote_ip" => remote,
                        );
                        act.id = res.expect("Error getting session")
                    },
                    Err(err) => {
                        error!(act.log.log,
                            "Unhandled Error: {:?}", err;
                            "remote_ip" => remote,
                            );
                        ctx.stop()
                    },
                }
                fut::ready(())
            })
            .wait(ctx);
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        // notify chat server
        debug!(
            self.log.log,
            "Killing session";
            "session" => &self.id,
            "remote_ip" => &self.meta.remote,
        );
        self.addr.do_send(server::Disconnect {
             channel: self.channel,
             id: self.id,
            reason: server::DisconnectReason::None,
        });
        Running::Stop
    }
}

/// Handle messages from chat server, we simply send it to peer websocket
impl Handler<server::Message> for WsChannelSession {
    type Result = ();

    fn handle(&mut self, msg: server::Message, ctx: &mut Self::Context) {
        match msg.0 {
            server::MessageType::Terminate => {
                debug!(
                    self.log.log,
                    "Closing session";
                    "session" => &self.id,
                    "remote_ip" => &self.meta.remote,
                );
                ctx.stop();
            },
            server::MessageType::Text => ctx.text(msg.1),
        }
    }
}

/// WebSocket message handler
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsChannelSession {
    fn handle(
        &mut self,
        msg: Result<ws::Message, ws::ProtocolError>,
        ctx: &mut Self::Context,
    ) {
        debug!(
            self.log.log,
            "Websocket Message: {:?}", msg;
            "remote_ip" => &self.meta.remote
        );
        let msg = match msg {
            Err(_) => {
                ctx.stop();
                return;
            }
            Ok(msg) => msg,
        };
        debug!(
            self.log.log,
            "WEBSOCKET MESSAGE: {:?}", msg;
            "remote_ip" => &self.meta.remote
        );
        match msg {
            ws::Message::Ping(msg) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            ws::Message::Pong(_) => {
                self.hb = Instant::now();
            }
            ws::Message::Text(text) => {
                self.hb = Instant::now();
                let m = text.trim();
                self.addr.do_send(server::ClientMessage {
                    id: self.id,
                    message_type: server::MessageType::Text,
                    msg: m.to_owned(),
                    channel: self.channel.clone(),
                    sender: self.meta.clone()
                })

            }
            ws::Message::Binary(_) => info!(
                self.log.log,
                "Unexpected binary";
                "remote_ip" => &self.meta.remote,
            ),
            ws::Message::Close(_) => {
                self.addr.do_send(server::Disconnect{
                    id: self.id,
                    channel: self.channel,
                    reason: server::DisconnectReason::None
                });
                debug!(
                    self.log.log,
                    "Shutting down session";
                    "session" => &self.id,
                    "remote_ip" => &self.meta.remote,
                );
                ctx.stop();
            }
            ws::Message::Continuation(_) => {
                ctx.stop();
            }
            ws::Message::Nop => (),
        }
    }
}

impl WsChannelSession {
    /// helper method that sends ping to client every second.
    ///
    /// also this method checks heartbeats from client
    fn hb(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // check client heartbeats
            if Instant::now().duration_since(act.hb) > CLIENT_TIMEOUT {
                // heartbeat timed out
                info!(
                    act.log.log,
                    "Client connected too long";
                    "session" => &act.id,
                    "channel" => &act.channel.to_string(),
                    "remote_ip" => &act.meta.remote,
                );

                // notify chat server
                act.addr.do_send(server::Disconnect {
                    id: act.id,
                    channel: act.channel.clone(),
                    reason: server::DisconnectReason::Timeout
                });
                // act.metrics.incr("conn.expired").unwrap();

                // stop actor
                ctx.stop();
                return;
            }
            if Instant::now().duration_since(act.hb)
                > act.expiry
                {
                    info!(
                        act.log.log,
                        "Client time-out. Disconnecting";
                        "session" => &act.id,
                        "channel" => &act.channel.to_string(),
                        "remote_ip" => &act.meta.remote,
                    );
                    // act.metrics.incr("conn.timeout").unwrap();
                    act.addr.do_send(server::Disconnect{
                        id: act.id,
                        channel: act.channel.clone(),
                        reason: server::DisconnectReason::Timeout,
                    });
                    ctx.stop();
                    return;
                }
            // Send the ping.
            ctx.ping(b"");
        });
    }
}

pub struct Server;

#[actix_rt::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();

    let settings = settings::Settings::new().unwrap();
    let sys = System::new("channelserver");
    let addr = format!("{}:{}", settings.hostname, settings.port);
    let log = if settings.human_logs{
        logging::MozLogger::new_human()
    } else {
        logging::MozLogger::new_json()
    };
    let mut trusted_list: Vec<ipnet::IpNet> = Vec::new();
    let server = server::ChannelServer::new(&settings, &log).start();

    // Add the known private networks to the trusted proxy list
    trusted_list.push("10.0.0.0/8".parse().unwrap());
    trusted_list.push("172.16.0.0/12".parse().unwrap());
    trusted_list.push("192.168.0.0/16".parse().unwrap());

    if !settings.trusted_proxy_list.is_empty() {
        for mut proxy in settings.trusted_proxy_list.split(',') {
            proxy = proxy.trim();
            if !proxy.is_empty() {
                let mut fixed = proxy.to_owned();
                if !proxy.contains('/') {
                    fixed = format!("{}/32", proxy);
                    debug!(log.log, "Fixing single address {}", fixed);
                }
                match fixed.parse::<ipnet::IpNet>() {
                    Ok(addr) => trusted_list.push(addr),
                    Err(err) => {
                        error!(log.log, r#"Ignoring unparsable IP address "{} {:?}"#, proxy, err);
                    }
                };
            }
        }
    }

    debug!(&log.log, "Trusted Proxies: {:?}", trusted_list);

    if !Path::new(&settings.mmdb_loc).exists() {
        error!(&log.log, "Cannot find geoip database: {}", settings.mmdb_loc);
        return Err(
            std::io::Error::new(std::io::ErrorKind::NotFound, "missing geoip database".to_owned())
        )
    };
    let db_loc = settings.mmdb_loc.clone();


    // Create Http server with websocket support
    HttpServer::new(move || {
        let logging = if settings.human_logs {
            logging::MozLogger::new_human()
        } else {
            logging::MozLogger::new_json()
        };
        App::new()
            .data(server.clone())
            // redirect to websocket.html
            .service(web::resource("/").route(web::get().to(|| {
                HttpResponse::Found()
                    .header("LOCATION", "/static/websocket.html")
                    .finish()
            })))
            // websocket
            .service(web::resource("/ws/").to(chat_route))
            // static resources
            .service(web::resource("/__heartbeat__").route(web::get().to(heartbeat)))
            .service(web::resource("/__lbheartbeat__").route(web::get().to(lbheartbeat)))
            .service(web::resource("/__version__").route(web::get().to(show_version)))

            .service(fs::Files::new("/static/", "static/"))
    })
    .bind(addr)?
    .run()
    .await
}
