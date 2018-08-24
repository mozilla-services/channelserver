//! `MessageRouter` is an actor. It retains a list of active websocket
//! actors accepting messages to send out. HTTP requests use this to
//! send messages to active websocket clients.

use std::cell::RefCell;
use std::collections::HashMap;

use actix::{Actor, Addr, Context, Handler};
use rand::{self, Rng, ThreadRng};
use uuid::Uuid;

use session;

/// New websocket client has identified itself
#[derive(Message)]
#[rtype(usize)]
pub struct Identify {
    pub id: Uuid,
    pub addr: Addr<session::ClientSession>,
}

/// Disconnect a websocket client that identified
#[derive(Message)]
pub struct Disconnect {
    pub id: (Uuid, usize),
}

/// Send a message to a websocket client
#[derive(Message)]
#[rtype(bool)]
pub struct ClientSend {
    pub id: Uuid,
    pub msg: String,
}

pub struct MessageRouter {
    clients: HashMap<Uuid, (Addr<session::ClientSession>, usize)>,
    rng: RefCell<ThreadRng>,
}

impl Default for MessageRouter {
    fn default() -> MessageRouter {
        MessageRouter {
            clients: HashMap::new(),
            rng: RefCell::new(rand::thread_rng()),
        }
    }
}

impl Actor for MessageRouter {
    type Context = Context<Self>;
}

impl Handler<Identify> for MessageRouter {
    type Result = usize;

    fn handle(&mut self, msg: Identify, _: &mut Context<Self>) -> Self::Result {
        // generate a unique session id for every client
        let session_id = self.rng.borrow_mut().gen::<usize>();

        // register the session, disconnect an older one if needed
        if let Some((addr, _)) = self.clients.insert(msg.id, (msg.addr, session_id)) {
            addr.do_send(session::DropConnection);
        }

        // return the clients session id
        session_id
    }
}

/// Remove a registered client when they disconnect
impl Handler<Disconnect> for MessageRouter {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        let client_exists = self
            .clients
            .get(&msg.id.0)
            .map_or(false, |client| client.1 == msg.id.1);
        if client_exists {
            self.clients.remove(&msg.id.0);
        }
    }
}

/// Send a message to a connected websocket client
impl Handler<ClientSend> for MessageRouter {
    type Result = bool;

    fn handle(&mut self, msg: ClientSend, _: &mut Context<Self>) -> Self::Result {
        if let Some((addr, _)) = self.clients.get(&msg.id) {
            addr.do_send(session::Message(msg.msg));
            true
        } else {
            false
        }
    }
}
