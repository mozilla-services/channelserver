use std::sync::Arc;
use std::time::{Duration, Instant};

use cadence::{CountedExt, StatsdClient, Timed};
use ipnet::IpNet;
use slog::{debug, error, info};

use actix::{
    Actor, ActorContext, ActorFutureExt, Addr, AsyncContext, ContextFutureSpawner, Handler,
    Running, StreamHandler, WrapFuture, fut,
};
use actix_web_actors::ws;

use crate::channelid;
use crate::logging;
use crate::meta;
use crate::server;
use crate::settings;
use crate::{CLIENT_TIMEOUT, HEARTBEAT_INTERVAL};

pub struct WsChannelSessionState {
    pub log: logging::MozLogger,
    pub metrics: Arc<StatsdClient>,
    pub settings: settings::Settings,
    pub iploc: maxminddb::Reader<Vec<u8>>,
    pub trusted_proxy_list: Vec<IpNet>,
}

impl std::fmt::Debug for WsChannelSessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "WsChannelSessionState{{ log: {:?}, metrics: {:?}, settings: {:?}, iploc: ..., trusted_proxy_list: {:?}}}",
            self.log, self.metrics, self.settings, self.trusted_proxy_list,
        )
    }
}

impl WsChannelSessionState {
    pub fn new(
        settings: &settings::Settings,
        log: &logging::MozLogger,
        metrics: &Arc<StatsdClient>,
    ) -> Self {
        let iploc = maxminddb::Reader::open_readfile(&settings.mmdb_loc).unwrap_or_else(|_| {
            panic!(
                "Could not find mmdb file at {:?}/{}",
                std::env::current_dir().unwrap().as_path().to_string_lossy(),
                settings.mmdb_loc
            )
        });

        let mut trusted_list: Vec<IpNet> = Vec::new();
        trusted_list.push("10.0.0.0/8".parse().unwrap());
        trusted_list.push("172.16.0.0/12".parse().unwrap());
        trusted_list.push("192.168.0.0/16".parse().unwrap());

        if !settings.trusted_proxy_list.is_empty() {
            for mut proxy in settings.trusted_proxy_list.split(',') {
                proxy = proxy.trim();
                if !proxy.is_empty() {
                    let mut fixed = proxy.to_owned();
                    if !proxy.contains('/') {
                        fixed = format!("{proxy}/32");
                        debug!(log.log, "Fixing single address {fixed}");
                    }
                    match fixed.parse::<ipnet::IpNet>() {
                        Ok(addr) => trusted_list.push(addr),
                        Err(err) => {
                            error!(
                                log.log,
                                r#"Ignoring unparsable IP address "{proxy} {err:?}"#
                            );
                        }
                    };
                }
            }
        }
        WsChannelSessionState {
            log: log.clone(),
            metrics: metrics.clone(),
            settings: settings.clone(),
            trusted_proxy_list: trusted_list,
            iploc,
        }
    }
}

pub struct WsChannelSession {
    /// unique session id
    pub id: usize,
    /// Client must send ping at least once per CLIENT_TIMEOUT seconds,
    /// otherwise we drop connection.
    pub hb: Instant,
    // max channel lifespan
    pub expiry: Duration,
    /// joined channel
    pub channel: channelid::ChannelID,
    /// is the first time we're connecting?
    pub initial_connection: bool,
    /// peer name
    pub meta: meta::SenderData,
    /// Address wrapper for Channel server
    pub addr: Addr<server::ChannelServer>,
    /// logging pointer
    pub log: logging::MozLogger,
    /// metrics reporting pointer
    pub metrics: Arc<cadence::StatsdClient>,
}

impl Actor for WsChannelSession {
    type Context = ws::WebsocketContext<Self>;

    /// Method is called on actor start.
    /// We register ws session with server
    fn started(&mut self, ctx: &mut Self::Context) {
        // we'll start heartbeat process on session start.
        self.hb(ctx);

        let meta = self.meta.clone();

        // register self in server. `AsyncContext::wait` register
        // future within context, but context waits until this future resolves
        // before processing any other events.
        // HttpContext::state() is instance of ChannelServer, state is shared
        // across all routes within application
        let addr = ctx.address();
        self.addr
            .send(server::Connect {
                addr: addr.recipient(),
                channel: self.channel,
                initial_connect: self.initial_connection,
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
                        let _ = act.metrics.incr("conn.create");
                        debug!(
                            act.log.log,
                            "Starting new session";
                            "session" => session_id,
                            "remote_ip" => remote,
                        );
                        act.id = res.expect("Error getting session")
                    }
                    Err(err) => {
                        error!(act.log.log,
                        "Unhandled Error: {:?}", err;
                        "remote_ip" => remote,
                        );
                        ctx.stop()
                    }
                }
                fut::ready(())
            })
            .wait(ctx);
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        // notify other channels that things are stopping
        debug!(
            self.log.log,
            "Killing session";
            "session" => &self.id,
            "remote_ip" => &self.meta.remote,
        );
        let _ = self.metrics.time(
            "conn.length",
            Instant::now().duration_since(self.hb).as_millis() as u64,
        );
        self.addr.do_send(server::Disconnect {
            channel: self.channel,
            id: self.id,
            reason: server::DisconnectReason::None,
        });
        Running::Stop
    }
}

impl Handler<server::TextMessage> for WsChannelSession {
    type Result = ();

    fn handle(&mut self, msg: server::TextMessage, ctx: &mut Self::Context) {
        match msg.0 {
            server::MessageType::Terminate => {
                debug!(
                    self.log.log,
                    "Closing session";
                    "session" => &self.id,
                    "remote_ip" => &self.meta.remote,
                );
                ctx.stop();
            }
            server::MessageType::Text => ctx.text(msg.1),
        }
    }
}

/// WebSocket message handler
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsChannelSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
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
                    channel: self.channel,
                    sender: self.meta.clone(),
                })
            }
            ws::Message::Binary(_) => info!(
                self.log.log,
                "Unexpected binary";
                "remote_ip" => &self.meta.remote,
            ),
            ws::Message::Close(_) => {
                self.addr.do_send(server::Disconnect {
                    id: self.id,
                    channel: self.channel,
                    reason: server::DisconnectReason::None,
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
                    "channel" => &act.channel.as_string(),
                    "remote_ip" => &act.meta.remote,
                );

                // notify server
                act.addr.do_send(server::Disconnect {
                    id: act.id,
                    channel: act.channel,
                    reason: server::DisconnectReason::Timeout,
                });
                act.metrics.incr("conn.expired").ok();

                // stop actor
                ctx.stop();
                return;
            }
            if Instant::now().duration_since(act.hb) > act.expiry {
                info!(
                    act.log.log,
                    "Client time-out. Disconnecting";
                    "session" => &act.id,
                    "channel" => &act.channel.as_string(),
                    "remote_ip" => &act.meta.remote,
                );
                act.metrics.incr("conn.timeout").ok();
                act.addr.do_send(server::Disconnect {
                    id: act.id,
                    channel: act.channel,
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
