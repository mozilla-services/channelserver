//! `ChannelServer` is an actor. It maintains list of connection client session.
//! And manages available channels. Peers send messages to other peers in same
//! channels through `ChannelServer`.
use std::collections::{hash_map::Entry, HashMap};
use std::fmt;
use std::time::Instant;

use actix::prelude::{Actor, Context, Handler, Message, MessageResult, Recipient};
use cadence::{Counted, StatsdClient};
use rand::{self, rngs::ThreadRng, Rng};
use serde::Serialize;
use serde_json::json;
use slog::{debug, error, trace, warn};

use crate::channelid::ChannelID;
use crate::error as perror;
use crate::logging;
use crate::logging::MozLogger;
use crate::meta;
use crate::metrics;
use crate::settings::Settings;

pub const EOL: &str = "\x04";

#[derive(Serialize, Debug, PartialEq)]
pub enum MessageType {
    Text,
    Terminate,
}

/// New session is created
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<TextMessage>,
    pub channel: ChannelID,
    pub remote: Option<String>,
    pub initial_connect: bool,
}

/// Session is disconnected
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub channel: ChannelID,
    pub id: SessionId,
    pub reason: DisconnectReason,
}

#[derive(Serialize, Debug, PartialEq, PartialOrd)]
pub enum DisconnectReason {
    None,
    _ConnectionError,
    Timeout,
}

impl fmt::Display for DisconnectReason {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                DisconnectReason::None => "Client Disconnect",
                DisconnectReason::_ConnectionError => "Connection Error",
                DisconnectReason::Timeout => "Connection Timeout",
            }
        )
    }
}

type Channels = HashMap<SessionId, Channel>;
type SessionId = usize;

#[derive(Message)]
#[rtype(result = "()")]
pub struct TextMessage(pub MessageType, pub String);

/// Send message to specific room
#[derive(Message)]
#[rtype(result = "()")]
pub struct ClientMessage {
    /// Id of the client session
    pub id: SessionId,
    // Type of message being sent
    pub message_type: MessageType,
    /// Peer message
    pub msg: String,
    /// channel name
    pub channel: ChannelID,
    /// Sender info
    pub sender: meta::SenderData,
}

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct Channel {
    pub session_id: SessionId,
    pub started: Instant,
    pub msg_count: u8,
    pub data_exchanged: usize,
    pub remote: Option<String>,
}

/// List of available rooms
pub struct ListChannels;

impl actix::Message for ListChannels {
    type Result = Vec<ChannelID>;
}

/// `ChannelServer` manages channels and is responsible for coordinating
/// sessions.
pub struct ChannelServer {
    // collections of sessions grouped by channel
    channels: HashMap<ChannelID, Channels>,
    // individual connections
    sessions: HashMap<SessionId, Recipient<TextMessage>>,
    // random number generator
    rng: ThreadRng,
    // logging object
    pub log: MozLogger,
    // configuration options
    pub settings: Settings,
    pub metrics: StatsdClient,
}

impl ChannelServer {
    pub fn new(settings: &Settings, log: &MozLogger) -> Self {
        let metrics = metrics::metrics_from_opts(settings, log).expect("Could not create metrics");
        // Add the known private networks to the trusted proxy list

        Self {
            sessions: HashMap::new(),
            channels: HashMap::new(),
            rng: ThreadRng::default(),
            log: log.clone(),
            settings: settings.clone(),
            metrics,
        }
    }

    /// Send message to all users in the room
    fn send_message(
        &mut self,
        channel: &ChannelID,
        message: &str,
        skip_id: SessionId,
    ) -> Result<(), perror::HandlerError> {
        if let Some(participants) = self.channels.get_mut(channel) {
            for party in participants.values_mut() {
                let max_data: usize = self.settings.max_data as usize;
                let msg_len = message.len();
                let remote_ip = party.remote.clone().unwrap_or_else(|| "Unknown".to_owned());
                if max_data > 0 && (party.data_exchanged > max_data || msg_len > max_data) {
                    warn!(
                        self.log.log,
                        "Too much data sent through {}, closing", channel;
                        "remote_ip" => &remote_ip
                    );
                    self.metrics.incr("conn.max.data").ok();
                    let mut remote = "";
                    if let Some(ref rr) = party.remote {
                        remote = rr;
                    }
                    return Err(perror::HandlerErrorKind::XSDataErr(remote.to_owned()).into());
                }
                party.data_exchanged += msg_len;
                let msg_count = self.settings.max_exchanges;
                party.msg_count += 1;
                if msg_count > 0 && party.msg_count > msg_count {
                    warn!(
                        self.log.log,
                        "Too many messages through {}, closing", channel;
                        "remote_ip" => &remote_ip
                    );
                    let mut remote = "";
                    if let Some(ref rr) = party.remote {
                        remote = rr;
                    }
                    self.metrics.incr("conn.max.msg").ok();
                    return Err(perror::HandlerErrorKind::XSMessageErr(remote.to_owned()).into());
                }
                if party.session_id != skip_id {
                    if let Some(addr) = self.sessions.get(&party.session_id) {
                        addr.do_send(TextMessage(MessageType::Text, message.to_owned()))
                            .ok();
                    }
                }
            }
        }
        Ok(())
    }

    fn disconnect(&mut self, channel: &ChannelID, id: usize) {
        if let Some(participants) = self.channels.get_mut(channel) {
            for pid in participants.keys() {
                if id == *pid {
                    debug!(self.log.log, "Sending disconnect to {}", pid);
                    if let Some(addr) = self.sessions.get(&id) {
                        // send a control message to force close
                        addr.do_send(TextMessage(MessageType::Terminate, EOL.to_owned()))
                            .ok();
                    }
                }
            }
        }
        let mut do_shutdown = false;
        if let Some(participants) = self.channels.get_mut(channel) {
            participants.remove(&id);
            if participants.is_empty() {
                do_shutdown = true;
            }
        }
        if do_shutdown {
            self.shutdown(channel);
        }
    }

    /// Kill a channel and terminate all participants.
    ///
    /// This sends a Terminate to each participant, which forces the connection closed.
    fn shutdown(&mut self, channel: &ChannelID) {
        if let Some(participants) = self.channels.get(channel) {
            for id in participants.keys() {
                if let Some(addr) = self.sessions.get(&id) {
                    // send a control message to force close
                    addr.do_send(TextMessage(MessageType::Terminate, EOL.to_owned()))
                        .ok();
                }
                self.sessions.remove(&id);
            }
        }
        debug!(self.log.log, "Removing channel {}", channel);
        self.channels.remove(channel);
    }
}

/// Is a previously connected client trying to reconnect?
fn reconnect_check(
    group: &Channels,
    new_remote: &Option<String>,
    log: Option<&logging::MozLogger>,
) -> bool {
    if let Some(ref req_ip) = new_remote {
        for participant in group.values() {
            if let Some(log) = log {
                debug!(log.log, "Checking {:?}", &participant.remote);
            }
            if let Some(ref loc_ip) = &participant.remote {
                if req_ip == loc_ip {
                    return true;
                }
            }
        }
    }
    false
}

/// Handler for Disconnect message.
impl Handler<Disconnect> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: Disconnect, _ctx: &mut Context<Self>) {
        debug!(
            self.log.log,
            "Connection dropped";
            "channel" => &msg.channel.as_string(),
            "session" => &msg.id,
            "reason" => format!("{}", &msg.reason),
        );
        self.disconnect(&msg.channel, msg.id);
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        if msg.message_type == MessageType::Terminate {
            return self.disconnect(&msg.channel, msg.id);
        }
        if self
            .send_message(
                &msg.channel,
                &json!({
                    "message": &msg.msg,
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

    fn handle(&mut self, msg: Connect, _ctx: &mut Context<Self>) -> Self::Result {
        let session_id = self.rng.gen::<usize>();
        let remote = &msg.remote.clone().unwrap_or_else(|| "Unkown".to_owned());
        let chan_id = &msg.channel.as_string();
        let new_session = Channel {
            session_id,
            started: Instant::now(),
            msg_count: 0,
            data_exchanged: 0,
            remote: msg.remote.clone(),
        };
        self.sessions
            .insert(new_session.session_id, msg.addr.clone());
        debug!(
            self.log.log,
            "New connection";
            "channel" => chan_id,
            "session" => &new_session.session_id,
            "remote_ip" => remote,
        );
        // Is this a new channel request?
        if let Entry::Vacant(entry) = self.channels.entry(msg.channel) {
            // Is this the first time we're requesting this channel?
            if !&msg.initial_connect {
                warn!(
                    self.log.log,
                    "Attempt to connect to unknown channel";
                    "channel" => chan_id,
                    "remote_ip" => remote,
                );
                return 0;
            }
            entry.insert(HashMap::new());
        };
        let group = match self.channels.get_mut(&msg.channel) {
            None => {
                trace!(self.log.log,
                "No group information found for channel";
                "channel" => chan_id,
                "remote_ip" => remote);
                return 0;
            }
            Some(v) => v,
        };
        if group.len() >= self.settings.max_channel_connections.into() {
            warn!(
                self.log.log,
                "Too many connections requested for channel";
                "channel" => chan_id,
                "remote_ip" => remote,
            );
            self.sessions.remove(&new_session.session_id);
            self.metrics.incr("conn.max.conn").ok();
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
        if group.len() > 2 && !reconnect_check(&group, &new_session.remote, Some(&self.log)) {
            error!(
                self.log.log,
                "Unexpected remote connection";
                "remote_ip" => remote,
            );
            return 0;
        };
        debug!(self.log.log,
            "Adding session to channel";
            "channel" => chan_id,
            "session" => &new_session.session_id,
            "remote_ip" => remote,
        );
        group.insert(session_id, new_session);
        // tell the client what their channel is.
        let jpath = json!({ "link": format!("/v1/ws/{}", chan_id),
                            "channelid": chan_id });
        if msg
            .addr
            .do_send(TextMessage(MessageType::Text, jpath.to_string()))
            .is_err()
        {
            warn!(
                self.log.log,
                "Could not send path to channel";
                "channel" => chan_id,
                "remote_ip" => remote
            )
        };
        session_id
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

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_reconnect() {
        let mut test_group: Channels = HashMap::new();

        test_group.insert(
            1,
            Channel {
                session_id: 1,
                started: Instant::now(),
                msg_count: 0,
                data_exchanged: 0,
                remote: Some("127.0.0.1".to_owned()),
            },
        );
        test_group.insert(
            2,
            Channel {
                session_id: 1,
                started: Instant::now(),
                msg_count: 0,
                data_exchanged: 0,
                remote: Some("127.0.0.2".to_owned()),
            },
        );

        assert!(reconnect_check(&test_group, &None, None) == false);
        assert!(reconnect_check(&test_group, &Some("10.0.0.1".to_owned()), None) == false);
        assert!(reconnect_check(&test_group, &Some("127.0.0.2".to_owned()), None) == true);
    }
}
