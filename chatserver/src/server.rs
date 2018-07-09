//! `ChatServer` is an actor. It maintains list of connection client session.
//! And manages available channels. Peers send messages to other peers in same
//! channel through `ChatServer`.

use actix::prelude::{Actor, Context, Handler, Recipient, Syn};
use rand::{self, Rng, ThreadRng};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use logging::MozLogger;

/// Chat server sends this messages to session
#[derive(Message)]
pub struct TextMessage(pub String);

/// Message for chat server communications

/// New chat session is created
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<Syn, TextMessage>,
    pub channel: Uuid,
}

/// Session is disconnected
#[derive(Message)]
pub struct Disconnect {
    pub channel: Uuid,
    pub id: usize,
}

/// Send message to specific channel
#[derive(Message)]
pub struct ClientMessage {
    /// Id of the client session
    pub id: usize,
    /// Peer message
    pub msg: String,
    /// channel name
    pub channel: Uuid,
}

/// `ChatServer` manages chat channels and responsible for coordinating chat
/// session. implementation is super primitive
pub struct ChatServer {
    sessions: HashMap<usize, Recipient<Syn, TextMessage>>,
    channels: HashMap<Uuid, HashSet<usize>>,
    rng: RefCell<ThreadRng>,
    log: MozLogger,
}

impl Default for ChatServer {
    fn default() -> ChatServer {
        ChatServer {
            sessions: HashMap::new(),
            channels: HashMap::new(),
            rng: RefCell::new(rand::thread_rng()),
            log: MozLogger::default(),
        }
    }
}

impl ChatServer {
    /// Send message to all users in the channel except skip_id
    fn send_message(&self, channel: &Uuid, message: &str, skip_id: usize) {
        if let Some(sessions) = self.channels.get(channel) {
            for id in sessions {
                if *id != skip_id {
                    if let Some(addr) = self.sessions.get(id) {
                        addr.do_send(TextMessage(message.to_owned())).unwrap_or(());
                    }
                }
            }
        }
    }
}

/// Make actor from `ChatServer`
impl Actor for ChatServer {
    /// We are going to use simple Context, we just need ability to communicate
    /// with other actors.
    type Context = Context<Self>;
}

/// Handler for Connect message.
///
/// Register new session and assign unique id to this session
impl Handler<Connect> for ChatServer {
    type Result = usize;

    fn handle(&mut self, msg: Connect, ctx: &mut Context<Self>) -> Self::Result {
        // notify all users in same channel
        // self.send_message(&"Main".to_owned(), "Someone joined", 0);

        // register session with random id
        let id = self.rng.borrow_mut().gen::<usize>();
        self.sessions.insert(id, msg.addr.clone());
        slog_debug!(
            self.log.log,
            "New connection to {} : {}",
            &msg.channel.simple(),
            &id
        );

        // auto join session to Main channel
        if !self.channels.contains_key(&msg.channel) {
            self.channels.insert(msg.channel, HashSet::new());
        }
        self.channels.get_mut(&msg.channel).unwrap().insert(id);
        // tell the client what their channel is.
        &msg.addr
            .do_send(TextMessage(format!("/ws/{}", &msg.channel.simple())));

        // send id back
        id
    }
}

/// Handler for Disconnect message.
impl Handler<Disconnect> for ChatServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        slog_debug!(
            self.log.log,
            "Connection dropped for {} : {}",
            &msg.channel.simple(),
            &msg.id
        );

        let mut channels: Vec<Uuid> = Vec::new();

        slog_debug!(
            self.log.log,
            "Session has id {}:{}",
            &msg.id,
            self.sessions.contains_key(&msg.id)
        );
        // remove address
        if self.sessions.remove(&msg.id).is_some() {
            // remove session from all channels
            for (chan, sessions) in &mut self.channels {
                if sessions.remove(&msg.id) {
                    channels.push(*chan);
                }
            }
        }
        // TODO: drop all other channels
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChatServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        self.send_message(&msg.channel, msg.msg.as_str(), msg.id);
    }
}
