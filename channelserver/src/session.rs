use std::time::Instant;

use actix::{
    fut, Actor, ActorContext, ActorFuture, Addr, AsyncContext, ContextFutureSpawner, Handler,
    Running, StreamHandler, WrapFuture,
};
use actix_web::ws;
use uuid::Uuid;

use logging;
use server;

/// This is our websocket route state, this state is shared with all route
/// instances via `HttpContext::state()`
pub struct WsChannelSessionState {
    pub addr: Addr<server::ChannelServer>,
    pub log: Addr<logging::MozLogger>,
}

pub struct WsChannelSession {
    /// unique session id
    pub id: usize,
    /// Client must send ping at least once per 10 seconds, otherwise we drop
    /// connection.
    pub hb: Instant,
    /// joined channel
    pub channel: Uuid,
    /// peer name
    pub name: Option<String>,
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
        let addr: Addr<Self> = ctx.address();
        ctx.state()
            .addr
            .send(server::Connect {
                addr: addr.recipient(),
                channel: self.channel.clone(),
            }).into_actor(self)
            .then(|res, act, ctx| {
                match res {
                    Ok(res) => {
                        ctx.state().log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Debug,
                            msg: format!("Starting new session [{}]", res),
                        });
                        act.id = res;
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
            }).wait(ctx);
    }

    fn stopping(&mut self, ctx: &mut Self::Context) -> Running {
        // notify chat server

        ctx.state().log.do_send(logging::LogMessage {
            level: logging::ErrorLevel::Debug,
            msg: format!("Killing session [{:?}]", self.id),
        });
        // Broadcast the close to all attached clients.
        ctx.state().addr.do_send(server::ClientMessage {
            id: 0,
            msg: "\x04".to_owned(),
            channel: self.channel.clone(),
        });

        Running::Stop
    }
}

/// Handle messages from chat server, we simply send it to peer websocket
impl Handler<server::TextMessage> for WsChannelSession {
    type Result = ();

    fn handle(&mut self, msg: server::TextMessage, ctx: &mut Self::Context) {
        if msg.0 == "\x04" {
            ctx.state().log.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Debug,
                msg: format!("Close recv'd for session [{:?}]", self.id),
            });
            ctx.close(None);
        } else {
            ctx.text(msg.0);
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
            ws::Message::Ping(msg) => ctx.pong(&msg),
            ws::Message::Pong(msg) => self.hb = Instant::now(),
            ws::Message::Text(text) => {
                let m = text.trim();
                // send message to chat server
                ctx.state().addr.do_send(server::ClientMessage {
                    id: self.id,
                    msg: m.to_owned(),
                    channel: self.channel.clone(),
                })
            }
            ws::Message::Binary(bin) => {
                ctx.state().log.do_send(logging::LogMessage {
                    level: logging::ErrorLevel::Info,
                    msg: format!("TODO: Binary format not yet supported"),
                });
            }
            ws::Message::Close(_) => {
                ctx.state().log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Debug,
                            msg: format!("Shutting down session [{}].", self.id),
                        });
                ctx.stop();
            }
        }
    }
}
