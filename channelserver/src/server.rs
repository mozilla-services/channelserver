//! `ChannelServer` is an actor. It maintains list of connection client session.
//! And manages available channels. Peers send messages to other peers in same
//! channel through `ChannelServer`.

// use std::sync::{Arc, Mutex};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

use actix::prelude::{Actor, Context, Handler, Recipient};
use rand::{self, Rng, ThreadRng};
use uuid::Uuid;

use logging::MozLogger;
use perror;
use settings::Settings;

//pub type ChannelCollection = Arc<Mutex<HashMap<Uuid, HashMap<usize, Channel>>>>;
//pub type SessionCollection = Arc<Mutex<HashMap<usize,Recipient<TextMessage>>>>;

/// Chat server sends this messages to session
#[derive(Message)]
pub struct TextMessage(pub String);

/// Message for chat server communications

/// New chat session is created
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<TextMessage>,
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

#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct Channel {
    pub id: usize,
    pub started: Instant,
    pub msg_count: u8,
    pub data_exchanged: usize,
}

/// `ChannelServer` manages chat channels and responsible for coordinating chat
/// session. implementation is super primitive
pub struct ChannelServer {
    me: usize, // id (to see if threading an issue)
    //channels: ChannelCollection, // collections of sessions grouped by channel
    //sessions: SessionCollection, // individual connections
    channels: HashMap<Uuid, HashMap<usize, Channel>>,
    sessions: HashMap<usize, Recipient<TextMessage>>,
    rng: RefCell<ThreadRng>,
    log: MozLogger,
    pub settings: RefCell<Settings>,
}

impl Default for ChannelServer {
    fn default() -> ChannelServer {
        let me = rand::thread_rng().gen::<usize>();
        ChannelServer {
            me: me,
            channels: HashMap::new(),
            sessions: HashMap::new(),
            rng: RefCell::new(rand::thread_rng()),
            log: MozLogger::default(),
            settings: RefCell::new(Settings::new().unwrap()),
        }
    }
}

/*
impl ChannelServer {
    pub fn init(channels:ChannelCollection,
                sessions:SessionCollection) -> ChannelServer {
        let me = rand::thread_rng().gen::<usize>();
        ChannelServer {
            me: me,
            channels: channels,
            sessions: sessions,
            rng: RefCell::new(rand::thread_rng()),
            log: MozLogger::default(),
            settings: RefCell::new(Settings::new().unwrap()),
        }
    }
}
*/

impl ChannelServer {
    /// Send message to all users in the channel except skip_id
    fn send_message(
        &mut self,
        channel: &Uuid,
        message: &str,
        skip_id: usize,
    ) -> Result<(), perror::HandlerError> {
        //if let Some(participants) = self.channels.lock().unwrap().get_mut(channel) {
        if let Some(participants) = self.channels.get_mut(channel) {
            // show's over, everyone go home.
            if message == "\04" {
                for (id, info) in participants {
                    if let Some(addr) = self.sessions.get(id) {
                        addr.do_send(TextMessage("\04".to_owned())).unwrap_or(());
                    }
                }
                return Ok(());
            }
            for party in participants.values_mut() {
                if party.started.elapsed().as_secs() > self.settings.borrow().timeout {
                    slog_info!(self.log.log, "Connection {} expired, closing", channel);
                    return Err(perror::HandlerErrorKind::ExpiredErr.into());
                }
                let max_data: usize = self.settings.borrow().max_data as usize;
                let msg_len = message.len();
                if max_data > 0 && party.data_exchanged > max_data - msg_len {
                    slog_info!(
                        self.log.log,
                        "Too much data sent through {}, closing",
                        channel
                    );
                    return Err(perror::HandlerErrorKind::XSDataErr.into());
                }
                party.data_exchanged += msg_len;
                let msg_count = self.settings.borrow().max_exchanges;
                if msg_count > 0 && party.msg_count > msg_count {
                    slog_info!(
                        self.log.log,
                        "Too many messages through {}, closing",
                        channel
                    );
                    return Err(perror::HandlerErrorKind::XSMessageErr.into());
                }
                party.msg_count += 1;
                if party.id != skip_id {
                    if let Some(addr) = self.sessions.get(&party.id) {
                        addr.do_send(TextMessage(message.to_owned())).unwrap_or(());
                    }
                }
            }
        }
        Ok(())
    }

    /// Kill a channel and terminate all participants.
    ///
    /// This sends a ^D message to each participant, which forces the connection closed.
    fn shutdown(&mut self, channel: &Uuid) {
        // let mut sessions = self.sessions.lock().unwrap();
        if let Some(participants) = self.channels.get_mut(channel) {
            for (id, info) in participants {
                if let Some(addr) = self.sessions.get(&id) {
                    // send a control message to force close
                    addr.do_send(TextMessage("\x04".to_owned())).unwrap_or(());
                }
                self.sessions.remove(&id);
            }
        }
    }
}

/// Make actor from `ChannelServer`
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
        // notify all users in same channel
        // self.send_message(&"Main".to_owned(), "Someone joined", 0);

        let id = self.rng.borrow_mut().gen::<usize>();
        let new_chan = Channel {
            // register session with random id
            id: id.clone(),
            started: Instant::now(),
            msg_count: 0,
            data_exchanged: 0,
        };
        {
            self.sessions.insert(new_chan.id, msg.addr.clone());
        }
        slog_debug!(
            self.log.log,
            "New connection to {} for  {} : [{}]",
            &self.me,
            &msg.channel.simple(),
            &new_chan.id
        );

        // auto join session to Main channel
        let chan_id = &msg.channel.simple();
        {
            // let mut channels = self.channels.lock().unwrap();
            if !self.channels.contains_key(&msg.channel) {
                slog_debug!(
                    self.log.log,
                    "Creating new channel set {}: [{}]",
                    chan_id,
                    &new_chan.id,
                );
                self.channels.insert(msg.channel, HashMap::new());
            } else {
                slog_debug!(
                    self.log.log,
                    "Adding session [{}] to existing channel set {}",
                    &new_chan.id,
                    chan_id
                )
            }
            let group = self
                .channels
                .get_mut(&msg.channel)
                .expect(&format!("Could not get channels for {}", &chan_id));
            group.insert(id.clone(), new_chan);
            slog_debug!(self.log.log, "channel {}: [{:?}]", chan_id, group,);
        }
        // tell the client what their channel is.
        &msg.addr.do_send(TextMessage(format!("/v1/ws/{}", chan_id)));

        // send id back
        id
    }
}

/// Handler for Disconnect message.
impl Handler<Disconnect> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, ctx: &mut Context<Self>) {
        slog_debug!(
            self.log.log,
            "Connection dropped for {} : {}",
            &msg.channel.simple(),
            &msg.id
        );
        slog_debug!(
            self.log.log,
            "Session had id {}:{}",
            &msg.id,
            self.sessions.contains_key(&msg.id)
        );
        self.shutdown(&msg.channel);
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        if self
            .send_message(&msg.channel, msg.msg.as_str(), msg.id)
            .is_err()
        {
            self.shutdown(&msg.channel)
        }
    }
}
