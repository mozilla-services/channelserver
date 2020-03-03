//! `ChatServer` is an actor. It maintains list of connection client session.
//! And manages available rooms. Peers send messages to other peers in same
//! room through `ChatServer`.
use std::collections::{HashMap, HashSet};
use std::fmt;

use actix::prelude::*;
use maxminddb;
use rand::{self, rngs::ThreadRng, Rng};
use serde::Serialize;

use crate::channelid;
use crate::error;
use crate::logging::MozLogger;
use crate::meta;
use crate::metrics;
use crate::settings::Settings;

/// Chat server sends this messages to session
#[derive(Message)]
#[rtype(result = "()")]
pub struct Message(pub MessageType, pub String);

pub const EOL: &str = "\x04";

#[derive(Serialize, Debug, PartialEq)]
pub enum MessageType {
    Text,
    Terminate,
}

/// Message for chat server communications

/// New chat session is created
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<Message>,
    pub remote: Option<String>,
    pub initial_connect: bool,
}

/// Session is disconnected
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub channel: channelid::ChannelID,
    pub id: usize,
    pub reason: DisconnectReason,
}

#[derive(Serialize, Debug, PartialEq, PartialOrd)]
pub enum DisconnectReason {
    None,
    ConnectionError,
    Timeout,
}

impl fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                DisconnectReason::None => "Client Disconnect",
                DisconnectReason::ConnectionError => "Connection Error",
                DisconnectReason::Timeout => "Connection Timeout",
            }
        )
    }
}

/// Send message to specific room
#[derive(Message)]
#[rtype(result = "()")]
pub struct ClientMessage {
    /// Id of the client session
    pub id: usize,
    /// Peer message
    pub msg: String,
    pub message_type: MessageType,
    pub channel: channelid::ChannelID,
    pub sender: meta::SenderData,
}

/// List of available rooms
pub struct ListChannels;

impl actix::Message for ListChannels {
    type Result = Vec<channelid::ChannelID>;
}

/// Join room, if room does not exists create new one.
#[derive(Message)]
#[rtype(result = "()")]
pub struct Join {
    /// Client id
    pub id: usize,
    /// Room name
    pub name: String,
}

/// `ChatServer` manages chat rooms and responsible for coordinating chat
/// session. implementation is super primitive
pub struct ChannelServer {
    sessions: HashMap<usize, Recipient<Message>>,
    channels: HashMap<channelid::ChannelID, HashSet<usize>>,
    rng: ThreadRng,

    log: MozLogger,
    iploc: maxminddb::Reader<Vec<u8>>,
    metrics: cadence::StatsdClient,
}

impl ChannelServer {
    pub fn new(settings: &Settings, log: &MozLogger) -> Self {
        let iploc = maxminddb::Reader::open_readfile(&settings.mmdb_loc)
            .expect("Could not read maxmind db");
        let metrics = metrics::metrics_from_opts(settings, log).expect("Could not create metrics");
        Self {
            sessions: HashMap::new(),
            channels: HashMap::new(),
            rng: ThreadRng::default(),
            log: log.clone(),
            iploc,
            metrics,
        }
    }

    /// Send message to all users in the room
    fn send_message(&self, channelid: &channelid::ChannelID, message: &str, skip_id: usize) {
        if let Some(sessions) = self.channels.get(channelid) {
            for id in sessions {
                if *id != skip_id {
                    if let Some(addr) = self.sessions.get(id) {
                        let _ = addr.do_send(Message(MessageType::Text, message.to_owned()));
                    }
                }
            }
        }
    }
}

/// Make actor from `ChatServer`
impl Actor for ChannelServer {
    /// We are going to use simple Context, we just need ability to communicate
    /// with other actors.
    type Context = Context<Self>;
}

/// Handler for Connect message.
///
/// Register new session and assign unique id to this session
impl Handler<Connect> for ChannelServer {
    type Result = usize;

    fn handle(&mut self, msg: Connect, ctx: &mut Context<Self>) -> Self::Result {
        print!("Someone joined");

        // notify all users in same room
        //self.send_message(&"Main".to_owned(), "Someone joined", 0);

        // register session with random id
        let id = self.rng.gen::<usize>();
        self.sessions.insert(id, msg.addr);

        // auto join session to Main room
        // self.channels.get_mut(&"Main".to_owned()).unwrap().insert(id);

        // send id back
        id
    }
}

/// Handler for Disconnect message.
impl Handler<Disconnect> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _: &mut Context<Self>) {
        println!("Someone disconnected");

        let mut channels: Vec<&channelid::ChannelID> = Vec::new();

        // remove address
        if self.sessions.remove(&msg.id).is_some() {
            // remove session from all rooms
            for (name, sessions) in &mut self.channels {
                if sessions.remove(&msg.id) {
                    channels.push(name);
                }
            }
        }
        // broadcast to other users
        /*
        for channel in channels {
            self.send_message(&channel, "Someone disconnected", 0);
        }
        */
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        self.send_message(&msg.channel, msg.msg.as_str(), msg.id);
    }
}

/// Handler for `ListChannels` message.
impl Handler<ListChannels> for ChannelServer {
    type Result = MessageResult<ListChannels>;

    fn handle(&mut self, _: ListChannels, _: &mut Context<Self>) -> Self::Result {
        let mut channels = Vec::new();

        for key in self.channels.keys() {
            channels.push(key.to_owned())
        }

        MessageResult(channels)
    }
}

/*
/// Join room, send disconnect message to old room
/// send join message to new room
impl Handler<Join> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: Join, _: &mut Context<Self>) {
        let Join { id, name } = msg;
        let mut channels = Vec::new();

        // remove session from all rooms
        for (n, sessions) in &mut self.channels {
            if sessions.remove(&id) {
                channels.push(n.to_owned());
            }
        }
        // send message to other users
        for channel in channels {
            self.send_message(&channel, "Someone disconnected", 0);
        }

        if self.channels.get_mut(&name).is_none() {
            self.channels.insert(name.clone(), HashSet::new());
        }
        self.send_message(&name, "Someone connected", id);
        self.rooms.get_mut(&name).unwrap().insert(id);
    }
}
*/
