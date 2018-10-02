use std::time::{Duration, Instant};

use actix::{
    fut, Actor, ActorContext, ActorFuture, Addr, AsyncContext, ContextFutureSpawner, Handler,
    Running, StreamHandler, WrapFuture,
};
use actix_web::ws;
// use cadence::{StatsdClient, Counted};
use ipnet::IpNet;
use maxminddb;
use uuid::Uuid;

use ip_rate_limit::IPReputation;
use logging;
use meta::SenderData;
use server;

//TODO: configs?
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// This is our websocket route state, this state is shared with all route
/// instances via `HttpContext::state()`
pub struct WsChannelSessionState {
    pub addr: Addr<server::ChannelServer>,
    pub log: Addr<logging::MozLogger>,
    pub iploc: maxminddb::Reader,
    // pub metrics: StatsdClient,
    pub trusted_proxy_list: Vec<IpNet>,
    pub ip_rep: Option<IPReputation>,
    pub connection_lifespan: u64,
    pub client_timeout: u64,
}

pub struct WsChannelSession {
    /// unique session id
    pub id: server::SessionId,
    /// Client must send ping at least once per 10 seconds, otherwise we drop
    /// connection.
    pub hb: Instant,
    /// Max channel lifespan
    pub expiry: Instant,
    /// joined channel
    pub channel: Uuid,
    /// peer name
    pub meta: SenderData,
}

impl Actor for WsChannelSession {
    type Context = ws::WebsocketContext<Self, WsChannelSessionState>;

    /// Method is called on actor start.
    /// We register ws session with ChannelServer
    fn started(&mut self, ctx: &mut Self::Context) {
        // register self in chat server. `AsyncContext::wait` register
        // future within context, but context waits until this future resolves
        // before processing any other events.
        // HttpContext::state() is instance of WsChatSessionState, state is shared
        // across all routes within application

        self.hb(ctx);

        self.meta = SenderData::from(ctx.request().clone());
        //Check that the remote IP isn't abusive:
        if let Some(remote) = self.meta.remote.clone() {
            // TODO: bleh, fix this to not clone.
            ctx.state()
                .ip_rep
                .clone()
                .map(|abuser| match abuser.is_abusive(&remote) {
                    Ok(false) => {}
                    Ok(true) => {
                        ctx.state().log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Info,
                            msg: format!("Blocking abusive IP address: {:?}", remote),
                        });
                        ctx.stop()
                    }
                    Err(err) => ctx.state().log.do_send(logging::LogMessage {
                        level: logging::ErrorLevel::Error,
                        msg: format!("IPRep reported error: {:?}", err),
                    }),
                });
        };
        let addr: Addr<Self> = ctx.address();
        ctx.state()
            .addr
            .send(server::Connect {
                addr: addr.recipient(),
                channel: self.channel.clone(),
                remote: self.meta.remote.clone(),
            })
            .into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(session_id) => {
                        if session_id == 0 {
                            ctx.stop();
                            return fut::err(());
                        }
                        ctx.state().log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Debug,
                            msg: format!("Starting new session [{:?}]", session_id),
                        });
                        // ctx.state().metrics.incr("conn.create").ok();
                        act.id = session_id;
                    }
                    // something is wrong with chat server
                    Err(err) => {
                        ctx.state().log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Error,
                            msg: format!("{:?}", err),
                        });
                        ctx.stop()
                    }
                }
                fut::ok(())
            })
            .wait(ctx);
    }

    /// Stop Session and alert all others in channel to shut down.
    fn stopping(&mut self, ctx: &mut Self::Context) -> Running {
        // notify chat server
        ctx.state().log.do_send(logging::LogMessage {
            level: logging::ErrorLevel::Debug,
            msg: format!("Killing session [{:?}]", self.id),
        });
        if self.id != 0 {
            // Broadcast the close to all attached clients.
            ctx.state().addr.do_send(server::ClientMessage {
                id: 0,
                message_type: server::MessageType::Terminate,
                message: server::EOL.to_owned(),
                channel: self.channel.clone(),
                sender: SenderData::default(),
                abuser: None,
            });
        }
        Running::Stop
    }
}

/// Handle messages from chat server, we simply send it to peer websocket
impl Handler<server::TextMessage> for WsChannelSession {
    type Result = ();

    fn handle(&mut self, msg: server::TextMessage, ctx: &mut Self::Context) {
        match msg.0 {
            server::MessageType::Terminate => {
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Debug,
                    msg: format!("Closing session [{:?}]", self.id),
                });
                ctx.close(Some(ws::CloseCode::Normal.into()));
            }
            server::MessageType::Text => ctx.text(msg.1),
        }
    }
}

/// WebSocket message handler
impl StreamHandler<ws::Message, ws::ProtocolError> for WsChannelSession {
    fn handle(&mut self, msg: ws::Message, ctx: &mut Self::Context) {
        ctx.state().log.do_send(logging::LogMessage {
            level: logging::ErrorLevel::Debug,
            msg: format!("Websocket Message: {:?}", msg),
        });
        match msg {
            ws::Message::Ping(msg) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            ws::Message::Pong(msg) => {
                self.hb = Instant::now();
            }
            ws::Message::Text(text) => {
                self.hb = Instant::now();
                let mut m = text.trim();
                ctx.state().addr.do_send(server::ClientMessage {
                    id: self.id,
                    message_type: server::MessageType::Text,
                    message: m.to_owned(),
                    channel: self.channel.clone(),
                    sender: self.meta.clone(),
                    abuser: ctx.state().ip_rep.clone(),
                })
            }
            ws::Message::Binary(bin) => {
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Info,
                    msg: format!("TODO: Binary format not supported"),
                });
            }
            ws::Message::Close(_) => {
                ctx.state().addr.do_send(server::Disconnect {
                    id: self.id,
                    channel: self.channel.clone(),
                    reason: server::DisconnectReason::None,
                });
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Debug,
                    msg: format!("Shutting down session [{}].", self.id),
                });
                ctx.stop();
            }
        }
    }
}

impl WsChannelSession {
    fn hb(&self, ctx: &mut ws::WebsocketContext<Self, WsChannelSessionState>) {
        ctx.run_interval(HEARTBEAT_INTERVAL, |act, ctx| {
            // Has the connection's lifespan expired?
            if Instant::now() > act.expiry {
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Warn,
                    msg: format!("Client connected too long. {}:{}", act.id, act.channel,),
                });
                ctx.state().addr.do_send(server::Disconnect {
                    id: act.id,
                    channel: act.channel.clone(),
                    reason: server::DisconnectReason::Timeout,
                });
                ctx.stop();
                return;
            }
            // Have we not gotten any traffic from the client?
            if Instant::now().duration_since(act.hb) > CLIENT_TIMEOUT {
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Warn,
                    msg: format!("Client time-out. Disconnecting {}:{}", act.id, act.channel,),
                });
                ctx.state().addr.do_send(server::Disconnect {
                    id: act.id,
                    channel: act.channel.clone(),
                    reason: server::DisconnectReason::ConnectionError,
                });
                ctx.stop();
                return;
            }
            // Send the ping.
            ctx.ping("");
        });
    }
}
