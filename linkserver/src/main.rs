#[macro_use]
extern crate actix;
extern crate actix_web;
extern crate futures;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate rand;
extern crate uuid;

use actix::{
    fut, Actor, ActorContext, ActorFuture, Addr, Arbiter, AsyncContext, ContextFutureSpawner,
    Handler, Running, StreamHandler, Syn, WrapFuture,
};
use actix_web::middleware::cors::Cors;
use actix_web::server::HttpServer;
use actix_web::{
    error, fs, http, ws, App, AsyncResponder, Error, HttpRequest, HttpResponse, Path, Responder,
    State,
};
use futures::Future;
use uuid::Uuid;

mod server;
mod session;

use session::ClientSession;

/// This is our websocket route state, this state is shared with all route
/// instances via `HttpContext::state()`
pub struct WsChatSessionState {
    addr: Addr<Syn, server::MessageRouter>,
}

/// Entry point for our route
fn message_route(req: HttpRequest<WsChatSessionState>) -> Result<HttpResponse, Error> {
    ws::start(req, ClientSession { id: None })
}

/// Client ID from the Path
#[derive(Deserialize)]
struct Info {
    clientid: Uuid,
}

/// HTTP entry point for message sending
fn message_post(data: (Path<Info>, String, State<WsChatSessionState>)) -> impl Responder {
    let (params, body, state) = data;
    state
        .addr
        .send(server::ClientSend {
            id: params.clientid,
            msg: body,
        })
        .then(|res| match res {
            Ok(true) => Ok(HttpResponse::Ok()),
            Ok(false) => Err(error::ErrorNotFound("Client is not connected")),
            Err(_) => Err(error::ErrorInternalServerError("Unable to process message")),
        })
        .responder()
}

impl Actor for ClientSession {
    type Context = ws::WebsocketContext<Self, WsChatSessionState>;

    fn stopping(&mut self, ctx: &mut Self::Context) -> Running {
        if let Some(id) = self.id {
            ctx.state().addr.do_send(server::Disconnect { id });
        }
        Running::Stop
    }
}

impl Handler<session::Message> for ClientSession {
    type Result = ();

    fn handle(&mut self, msg: session::Message, ctx: &mut Self::Context) {
        ctx.text(msg.0);
    }
}

impl Handler<session::DropConnection> for ClientSession {
    type Result = ();
    fn handle(&mut self, _: session::DropConnection, ctx: &mut Self::Context) {
        ctx.stop();
    }
}

/// Websocket Message Handler
///
/// The only valid message is a hex UUID after connect. All other messages
/// will result in the connection being dropped, including any after the
/// initial hex UUID.
///
/// The hex UUID will be echo'd back to confirm successful initiation.
impl StreamHandler<ws::Message, ws::ProtocolError> for ClientSession {
    fn handle(&mut self, msg: ws::Message, ctx: &mut Self::Context) {
        match msg {
            ws::Message::Ping(msg) => ctx.pong(&msg),
            ws::Message::Pong(_) => (),
            ws::Message::Text(text) => {
                let m = text.trim().to_owned();
                if self.id.is_some() {
                    ctx.stop();
                    return;
                }
                if let Ok(id) = Uuid::parse_str(&m) {
                    let addr: Addr<Syn, _> = ctx.address();
                    ctx.state()
                        .addr
                        .send(server::Identify { id, addr: addr })
                        .into_actor(self)
                        .then(move |res, act, ctx| {
                            match res {
                                Ok(res) => {
                                    act.id = Some((id, res));
                                    ctx.text(m);
                                }
                                _ => ctx.stop(),
                            }
                            fut::ok(())
                        })
                        .wait(ctx);
                } else {
                    ctx.stop();
                }
            }
            ws::Message::Binary(_) => ctx.stop(),
            ws::Message::Close(_) => {
                ctx.stop();
            }
        }
    }
}

fn main() {
    let sys = actix::System::new("linkserver");

    // Start message router actor in separate thread
    let server: Addr<Syn, _> = Arbiter::start(|_| server::MessageRouter::default());

    // Create Http server with websocket support
    HttpServer::new(move || {
        // Websocket sessions state
        let state = WsChatSessionState {
            addr: server.clone(),
        };

        App::with_state(state)
            // HTTP message resource, with CORS
            .configure(|app| {
                Cors::for_app(app)
                    .resource(
                        "/v1/send/{clientid}", |r| {
                            r.method(http::Method::PUT)
                                .with(message_post)
                    })
                    .register()
            })
            // websocket
            .resource("/ws/", |r| r.route().f(message_route))
            // static resources
            .handler("/", fs::StaticFiles::new("static/"))
    }).bind("127.0.0.1:8080")
        .unwrap()
        .start();

    println!("Started http server: 127.0.0.1:8080");
    let _ = sys.run();
}

#[cfg(test)]
mod tests {
    use actix_web::test;
    use futures::Stream;

    use super::*;

    fn build_srv() -> test::TestServer {
        test::TestServer::build_with_state(|| {
            let addr: Addr<Syn, _> = Arbiter::start(|_| server::MessageRouter::default());
            WsChatSessionState { addr }
        }).start(|app| {
            app.handler(|req| ws::start(req, ClientSession { id: None }));
            app.resource("/v1/send/{clientid}", |r| {
                r.method(http::Method::PUT).with(message_post)
            });
        })
    }

    #[test]
    fn test_relay_success() {
        let mut srv = build_srv();

        // Connect a websocket client and identify
        let uuid = Uuid::new_v4().simple().to_string();
        let (reader, mut writer) = srv.ws().unwrap();
        writer.text(uuid.clone());

        // Get the echo'd content back
        let (item, reader) = srv.execute(reader.into_future()).unwrap();
        assert_eq!(item, Some(ws::Message::Text(uuid.clone())));

        // Send in a test web request
        let path = format!("/v1/send/{}", &uuid);
        let request = srv
            .client(http::Method::PUT, &path)
            .body("hello client")
            .unwrap();
        let response = srv.execute(request.send()).unwrap();
        assert!(response.status().is_success());

        // Check the websocket content to match
        let (item, _reader) = srv.execute(reader.into_future()).unwrap();
        assert_eq!(item, Some(ws::Message::Text("hello client".to_string())));
    }
}
