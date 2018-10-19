//! `ChannelServer` is an actor. It maintains list of connection client session.
//! And manages available channels. Peers send messages to other peers in same
//! channel through `ChannelServer`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::time::Instant;

use actix::prelude::{Actor, Context, Handler, Recipient};
// use cadence::StatsdClient;
use rand::{self, Rng, ThreadRng};
use uuid::Uuid;

use logging::MozLogger;
use meta;
// use metrics;
use perror;
use settings::Settings;

pub const EOL: &'static str = "\x04";

#[derive(Serialize, Debug, PartialEq)]
pub enum MessageType {
    Text,
    Terminate,
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

/// Chat server sends this messages to session
#[derive(Message)]
pub struct TextMessage(pub MessageType, pub String);

/// Message for chat server communications
/// Individual session identifier
pub type SessionId = usize;
pub type ChannelId = usize;

/// New chat session is created
#[derive(Message)]
#[rtype(SessionId)]
pub struct Connect {
    pub addr: Recipient<TextMessage>,
    pub channel: Uuid,
    pub remote: Option<String>,
}

/// Session is disconnected
#[derive(Message)]
pub struct Disconnect {
    pub channel: Uuid,
    pub id: SessionId,
    pub reason: DisconnectReason,
}

/// Send message to specific channel
#[derive(Message, Serialize, Debug)]
pub struct ClientMessage {
    /// Id of the client session
    pub id: SessionId,
    // Type of message being sent
    pub message_type: MessageType,
    /// Peer message
    pub message: String,
    /// channel name
    pub channel: Uuid,
    /// Sender info
    pub sender: meta::SenderData,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct Channel {
    pub id: ChannelId,
    pub started: Instant,
    pub msg_count: u8,
    pub data_exchanged: usize,
    pub remote: Option<String>,
}

type Channels = HashMap<ChannelId, Channel>;

/// `ChannelServer` manages chat channels and responsible for coordinating chat
/// session. implementation is super primitive
pub struct ChannelServer {
    // collections of sessions grouped by channel
    channels: HashMap<Uuid, Channels>,
    // individual connections
    sessions: HashMap<SessionId, Recipient<TextMessage>>,
    // random number generator
    rng: RefCell<ThreadRng>,
    // logging object
    log: MozLogger,
    // configuration options
    pub settings: RefCell<Settings>,
    // pub metrics: RefCell<StatsdClient>,
}

impl Default for ChannelServer {
    fn default() -> ChannelServer {
        let settings = Settings::new().unwrap();
        let logger = MozLogger::default();
        // let metrics = metrics::metrics_from_opts(&settings.clone(), logger.clone()).unwrap();
        ChannelServer {
            channels: HashMap::new(),
            sessions: HashMap::new(),
            rng: RefCell::new(rand::thread_rng()),
            log: logger.clone(),
            settings: RefCell::new(settings.clone()),
            // metrics: RefCell::new(metrics.clone())
        }
    }
}

impl ChannelServer {
    /// Send message to all users in the channel except skip_id
    fn send_message(
        &mut self,
        channel: &Uuid,
        message: &str,
        skip_id: SessionId,
    ) -> Result<(), perror::HandlerError> {
        if let Some(participants) = self.channels.get_mut(channel) {
            for party in participants.values_mut() {
                let max_data: usize = self.settings.borrow().max_data as usize;
                let msg_len = message.len();
                if max_data > 0 && (party.data_exchanged > max_data || msg_len > max_data) {
                    info!(
                        self.log.log,
                        "Too much data sent through {}, closing", channel
                    );
                    // self.metrics.borrow().incr("conn.max.data").ok();
                    let mut remote = "";
                    if let Some(ref rr) = party.remote {
                        remote = rr;
                    }
                    return Err(perror::HandlerErrorKind::XSDataErr(remote.to_owned()).into());
                }
                party.data_exchanged += msg_len;
                let msg_count = u8::from(self.settings.borrow().max_exchanges);
                party.msg_count += 1;
                if msg_count > 0 && party.msg_count > msg_count {
                    info!(
                        self.log.log,
                        "Too many messages through {}, closing", channel
                    );
                    let mut remote = "";
                    if let Some(ref rr) = party.remote {
                        remote = rr;
                    }
                    // self.metrics.borrow().incr("conn.max.msg").ok();
                    return Err(perror::HandlerErrorKind::XSMessageErr(remote.to_owned()).into());
                }
                if party.id != skip_id {
                    if let Some(addr) = self.sessions.get(&party.id) {
                        addr.do_send(TextMessage(MessageType::Text, message.to_owned()))
                            .ok();
                    }
                }
            }
        }
        Ok(())
    }

    /// Kill a channel and terminate all participants.
    ///
    /// This sends a Terminate to each participant, which forces the connection closed.
    fn shutdown(&mut self, channel: &Uuid) {
        if let Some(participants) = self.channels.get_mut(channel) {
            for (id, info) in participants {
                if let Some(addr) = self.sessions.get(&id) {
                    // send a control message to force close
                    addr.do_send(TextMessage(MessageType::Terminate, EOL.to_owned()))
                        .ok();
                }
                self.sessions.remove(&id);
            }
        }
    }
}

/// Is a previously connected client trying to reconnect?
fn reconnect_check(group: &Channels, new_remote: &Option<String>) -> bool {
    if let Some(ref req_ip) = new_remote {
        for (_, participant) in group {
            println!("Checking {:?}", &participant.remote);
            if let Some(ref loc_ip) = &participant.remote {
                if req_ip == loc_ip {
                    return true;
                }
            }
        }
    }
    return false;
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
    type Result = SessionId;

    fn handle(&mut self, msg: Connect, ctx: &mut Context<Self>) -> Self::Result {
        let session_id = self.rng.borrow_mut().gen::<SessionId>();
        let new_chan = Channel {
            // register session with random id
            id: session_id.clone(),
            started: Instant::now(),
            msg_count: 0,
            data_exchanged: 0,
            remote: msg.remote,
        };
        self.sessions.insert(new_chan.id, msg.addr.clone());
        debug!(
            self.log.log,
            "New connection to {}: [{}]",
            &msg.channel.to_simple(),
            &new_chan.id
        );
        let chan_id = &msg.channel.to_simple();
        {
            if !self.channels.contains_key(&msg.channel) {
                debug!(
                    self.log.log,
                    "Creating new channel set {}: [{}]", chan_id, &new_chan.id,
                );
                self.channels.insert(msg.channel, HashMap::new());
            // self.metrics.borrow().incr("conn.new").ok();
            } else {
                debug!(
                    self.log.log,
                    "Adding session [{}] to existing channel set {}", &new_chan.id, chan_id
                );
                // self.metrics.borrow().incr("conn.joined").ok();
            }
            // we've already checked and created this, so calling unwrap
            // should be safe. Creating here hits lifetime exceptions as
            // well.
            let group = self.channels.get_mut(&msg.channel).unwrap();
            if group.len() >= self.settings.borrow().max_channel_connections.into() {
                info!(
                    self.log.log,
                    "Too many connections requested for channel {}", chan_id
                );
                self.sessions.remove(&new_chan.id);
                // self.metrics.borrow().incr("conn.max.conn").ok();
                // It doesn't make sense to impose a high penalty for this
                // behavior, but we may want to flag and log the origin
                // IP for later analytics.
                // We could also impose a tiny penalty on the IP (if possible)
                // which would minimally impact accidental occurances, but
                // add up for major infractors.
                return 0;
            }
            // The group should have two principle parties, the auth and supplicant
            // Any connection beyond that group should be checked to ensure it's
            // from a known IP. If a principle that only has one connection and it
            // drops, it is possible that it can't reconnect, but that's not a bad
            // thing. We should just let the connection expire as invalid so that
            // it's not stolen.
            if group.len() > 2 {
                if !reconnect_check(&group, &new_chan.remote) {
                    error!(
                        self.log.log,
                        "Unexpected remote connection from {}",
                        new_chan.remote.clone().unwrap()
                    );
                    return 0;
                }
            }
            group.insert(session_id.clone(), new_chan);
            debug!(self.log.log, "channel {}: [{:?}]", chan_id, group,);
        }
        // tell the client what their channel is.
        let jpath = json!({ "link": format!("/v1/ws/{}", chan_id),
                            "channelid": chan_id.to_string() });
        &msg.addr
            .do_send(TextMessage(MessageType::Text, jpath.to_string()));

        // send id back
        session_id
    }
}

/// Handler for Disconnect message.
impl Handler<Disconnect> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, ctx: &mut Context<Self>) {
        debug!(
            self.log.log,
            "Connection dropped for {} : {} {}",
            &msg.channel.to_simple(),
            &msg.id,
            &msg.reason,
        );
        self.shutdown(&msg.channel);
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        if &msg.message_type == &MessageType::Terminate {
            return self.shutdown(&msg.channel);
        }
        if self
            .send_message(
                &msg.channel,
                &json!({
                    "message": &msg.message,
                    "sender": &msg.sender,
                })
                .to_string(),
                msg.id,
            )
            .is_err()
        {
            self.shutdown(&msg.channel)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_reconnect() {
        let mut test_group: Channels = HashMap::new();

        test_group.insert(
            1,
            Channel {
                id: 1,
                started: Instant::now(),
                msg_count: 0,
                data_exchanged: 0,
                remote: Some("127.0.0.1".to_owned()),
            },
        );
        test_group.insert(
            2,
            Channel {
                id: 1,
                started: Instant::now(),
                msg_count: 0,
                data_exchanged: 0,
                remote: Some("127.0.0.2".to_owned()),
            },
        );

        assert!(reconnect_check(&test_group, &None) == false);
        assert!(reconnect_check(&test_group, &Some("10.0.0.1".to_owned())) == false);
        assert!(reconnect_check(&test_group, &Some("127.0.0.2".to_owned())) == true);
    }
}
