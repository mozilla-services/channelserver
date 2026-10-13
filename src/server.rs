//! `ChannelServer` is an actor. It maintains list of connection client session.
//! And manages available channels. Peers send messages to other peers in same
//! channels through `ChannelServer`.
use std::collections::{HashMap, hash_map::Entry};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use actix::prelude::{Actor, AsyncContext, Context, Handler, Message, MessageResult, Recipient};
use cadence::{CountedExt, StatsdClient};
use rand::{self, RngExt, rngs::ThreadRng};
use serde::Serialize;
use serde_json::json;
use slog::{debug, error, trace, warn};

use crate::channelid::ChannelID;
use crate::error as perror;
use crate::logging;
use crate::logging::MozLogger;
use crate::meta;
use crate::settings::Settings;

pub const EOL: &str = "\x04";

#[derive(Serialize, Debug, Eq, PartialEq)]
pub enum MessageType {
    Text,
    Terminate,
    /// Debug only: cut the socket without a close frame, as a network drop would.
    Drop,
}

/// Connect result for a resume into a channel that no longer exists.
pub const CHANNEL_GONE: usize = usize::MAX;

/// New session is created
#[derive(Message)]
#[rtype(usize)]
pub struct Connect {
    pub addr: Recipient<TextMessage>,
    pub channel: ChannelID,
    pub remote: Option<String>,
    pub initial_connect: bool,
    /// Token from an earlier connection that dropped, to take its place back.
    pub resume: Option<String>,
    /// The client asked to be held after a drop.
    pub resumable: bool,
}

/// Session is disconnected
#[derive(Message)]
#[rtype(result = "()")]
pub struct Disconnect {
    pub channel: ChannelID,
    pub id: SessionId,
    pub reason: DisconnectReason,
}

#[derive(Serialize, Debug, Eq, PartialEq, PartialOrd)]
pub enum DisconnectReason {
    None,
    _ConnectionError,
    Timeout,
    /// The client sent a close frame, so it is not coming back.
    Closed,
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
                DisconnectReason::Closed => "Client Close",
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
    /// The client asked to be held after a drop.
    pub resumable: bool,
    /// Lets this participant reconnect into its own place after a drop.
    pub resume_token: String,
    /// The token used for the last resume, kept until the client proves it got
    /// the new one, in case the resumed socket dropped before the greeting.
    pub prev_token: Option<String>,
    pub dropped_at: Option<Instant>,
    /// Messages for this participant while it is dropped, replayed on resume.
    pub pending: Vec<String>,
}

/// Debug only: close the newest participant of a channel, to exercise resume.
#[derive(Message)]
#[rtype(result = "bool")]
pub struct DropNewest(pub ChannelID);

impl Handler<DropNewest> for ChannelServer {
    type Result = bool;

    fn handle(&mut self, msg: DropNewest, _: &mut Context<Self>) -> bool {
        let newest = self
            .channels
            .get(&msg.0)
            .and_then(|parties| {
                parties
                    .values()
                    .filter(|p| p.dropped_at.is_none())
                    .max_by_key(|p| p.started)
            })
            .map(|p| p.session_id);
        match newest.and_then(|id| self.sessions.get(&id)) {
            Some(addr) => {
                addr.do_send(TextMessage(MessageType::Drop, EOL.to_owned()));
                true
            }
            None => false,
        }
    }
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
    pub metrics: Arc<StatsdClient>,
}

impl ChannelServer {
    pub fn new(
        settings: &Settings,
        log: &MozLogger,
        metrics: std::sync::Arc<StatsdClient>,
    ) -> Self {
        // Add the known private networks to the trusted proxy list

        Self {
            sessions: HashMap::new(),
            channels: HashMap::new(),
            rng: ThreadRng::default(),
            log: log.clone(),
            settings: settings.clone(),
            metrics: metrics.clone(),
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
                    if party.dropped_at.is_some() {
                        if party.pending.len() >= self.settings.resume_buffer as usize {
                            self.metrics.incr("conn.resume.overflow").ok();
                            return Err(perror::HandlerErrorKind::XSMessageErr(remote_ip).into());
                        }
                        party.pending.push(message.to_owned());
                    } else if let Some(addr) = self.sessions.get(&party.session_id) {
                        addr.do_send(TextMessage(MessageType::Text, message.to_owned()));
                    }
                }
            }
        }
        Ok(())
    }

    /// A dropped participant keeps its place for `resume_window` seconds, so a
    /// client whose socket died (e.g. a backgrounded mobile browser) can resume
    /// the same encrypted session instead of failing the pairing.
    fn disconnect(&mut self, channel: &ChannelID, id: usize, reason: &DisconnectReason) {
        if self.settings.resume_window > 0
            && *reason != DisconnectReason::Closed
            && let Some(party) = self.channels.get_mut(channel).and_then(|p| p.get_mut(&id))
            && party.resumable
        {
            if party.dropped_at.is_none() {
                party.dropped_at = Some(Instant::now());
                self.metrics.incr("conn.resume.held").ok();
            }
            self.sessions.remove(&id);
            return;
        }
        self.remove_participant(channel, id);
    }

    fn remove_participant(&mut self, channel: &ChannelID, id: usize) {
        if let Some(participants) = self.channels.get_mut(channel) {
            for pid in participants.keys() {
                if id == *pid {
                    debug!(self.log.log, "Sending disconnect to {}", pid);
                    if let Some(addr) = self.sessions.get(&id) {
                        // send a control message to force close
                        addr.do_send(TextMessage(MessageType::Terminate, EOL.to_owned()));
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

    /// Close channels whose dropped participant did not come back in time. The
    /// peer still waiting would otherwise never learn the pairing is dead.
    fn expire_dropped(&mut self) {
        let window = Duration::from_secs(self.settings.resume_window);
        let expired: Vec<ChannelID> = self
            .channels
            .iter()
            .filter(|(_, parties)| {
                parties
                    .values()
                    .any(|p| p.dropped_at.is_some_and(|at| at.elapsed() > window))
            })
            .map(|(channel, _)| *channel)
            .collect();
        for channel in expired {
            self.metrics.incr("conn.resume.expired").ok();
            self.shutdown(&channel);
        }
    }

    /// Kill a channel and terminate all participants.
    ///
    /// This sends a Terminate to each participant, which forces the connection closed.
    fn shutdown(&mut self, channel: &ChannelID) {
        if let Some(participants) = self.channels.get(channel) {
            for id in participants.keys() {
                if let Some(addr) = self.sessions.get(id) {
                    // send a control message to force close
                    addr.do_send(TextMessage(MessageType::Terminate, EOL.to_owned()));
                }
                self.sessions.remove(id);
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
    if let Some(req_ip) = new_remote {
        for participant in group.values() {
            if let Some(log) = log {
                debug!(log.log, "Checking {:?}", &participant.remote);
            }
            if let Some(loc_ip) = &participant.remote
                && req_ip == loc_ip
            {
                return true;
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
        self.disconnect(&msg.channel, msg.id, &msg.reason);
    }
}

/// Handler for Message message.
impl Handler<ClientMessage> for ChannelServer {
    type Result = ();

    fn handle(&mut self, msg: ClientMessage, _: &mut Context<Self>) {
        if msg.message_type == MessageType::Terminate {
            return self.disconnect(&msg.channel, msg.id, &DisconnectReason::Closed);
        }
        // The client sends only after it reads the greeting, so it has the new token.
        if let Some(party) = self
            .channels
            .get_mut(&msg.channel)
            .and_then(|p| p.get_mut(&msg.id))
        {
            party.prev_token = None;
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

    fn started(&mut self, ctx: &mut Self::Context) {
        if self.settings.resume_window > 0 {
            ctx.run_interval(
                Duration::from_secs(self.settings.heartbeat_interval.max(1)),
                |act, _| act.expire_dropped(),
            );
        }
    }
}

/// Handler for Connect message.
///
/// Register new session and assign unique id to this session
impl Handler<Connect> for ChannelServer {
    type Result = usize;

    fn handle(&mut self, msg: Connect, _ctx: &mut Context<Self>) -> Self::Result {
        let session_id = self.rng.random::<u64>() as usize;
        let remote = &msg.remote.clone().unwrap_or_else(|| "Unknown".to_owned());
        let chan_id = &msg.channel.as_string();
        let new_session = Channel {
            session_id,
            started: Instant::now(),
            msg_count: 0,
            data_exchanged: 0,
            remote: msg.remote.clone(),
            resumable: msg.resumable,
            resume_token: format!("{:032x}", self.rng.random::<u128>()),
            prev_token: None,
            dropped_at: None,
            pending: Vec::new(),
        };
        debug!(
            self.log.log,
            "New connection";
            "channel" => chan_id,
            "session" => &new_session.session_id,
            "remote_ip" => remote,
        );
        // A resume names an existing channel, so refuse it before one is created.
        if msg.resume.is_some() && msg.initial_connect {
            warn!(
                self.log.log,
                "Resume refused";
                "channel" => chan_id,
                "remote_ip" => remote,
            );
            self.metrics.incr("conn.resume.rejected").ok();
            return 0;
        }
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
                return if msg.resume.is_some() {
                    CHANNEL_GONE
                } else {
                    0
                };
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
        if let Some(token) = &msg.resume {
            // Only a participant the server saw drop can be resumed, so a leaked
            // token cannot kick a live client off its channel. The sweep runs
            // only once per heartbeat, so check the window here too.
            let window = Duration::from_secs(self.settings.resume_window);
            let Some(old_id) = group
                .iter()
                .find(|(_, p)| {
                    p.dropped_at.is_some_and(|at| at.elapsed() <= window)
                        && (&p.resume_token == token || p.prev_token.as_ref() == Some(token))
                })
                .map(|(id, _)| *id)
            else {
                warn!(
                    self.log.log,
                    "Resume refused";
                    "channel" => chan_id,
                    "remote_ip" => remote,
                );
                self.metrics.incr("conn.resume.rejected").ok();
                return 0;
            };
            let mut party = group.remove(&old_id).expect("participant just found");
            party.session_id = session_id;
            party.dropped_at = None;
            party.remote = msg.remote.clone();
            // A token works once, so one that leaks after use is worthless.
            party.prev_token = Some(token.clone());
            party.resume_token = format!("{:032x}", self.rng.random::<u128>());
            let pending = std::mem::take(&mut party.pending);
            let greeting = json!({ "link": format!("/v1/ws/{}", chan_id),
                                   "channelid": chan_id,
                                   "resume": &party.resume_token });
            group.insert(session_id, party);
            self.sessions.insert(session_id, msg.addr.clone());
            // do_send, as the session mailbox can hold fewer messages than the buffer.
            msg.addr
                .do_send(TextMessage(MessageType::Text, greeting.to_string()));
            for message in pending {
                msg.addr.do_send(TextMessage(MessageType::Text, message));
            }
            debug!(self.log.log,
                "Resumed session";
                "channel" => chan_id,
                "session" => session_id,
                "remote_ip" => remote,
            );
            self.metrics.incr("conn.resume.ok").ok();
            return session_id;
        }
        if group.len() >= self.settings.max_channel_connections as usize {
            warn!(
                self.log.log,
                "Too many connections requested for channel";
                "channel" => chan_id,
                "remote_ip" => remote,
            );
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
        if group.len() > 2 && !reconnect_check(group, &new_session.remote, Some(&self.log)) {
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
        let resume_token = new_session.resume_token.clone();
        let resumable = new_session.resumable;
        group.insert(session_id, new_session);
        self.sessions.insert(session_id, msg.addr.clone());
        // tell the client what their channel is, and how to resume it.
        let mut jpath = json!({ "link": format!("/v1/ws/{}", chan_id),
                                "channelid": chan_id });
        if self.settings.resume_window > 0 && resumable {
            jpath["resume"] = json!(resume_token);
        }
        if msg
            .addr
            .try_send(TextMessage(MessageType::Text, jpath.to_string()))
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
                resumable: false,
                resume_token: String::new(),
                prev_token: None,
                dropped_at: None,
                pending: Vec::new(),
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
                resumable: false,
                resume_token: String::new(),
                prev_token: None,
                dropped_at: None,
                pending: Vec::new(),
            },
        );

        assert!(!reconnect_check(&test_group, &None, None));
        assert!(!reconnect_check(
            &test_group,
            &Some("10.0.0.1".to_owned()),
            None
        ));
        assert!(reconnect_check(
            &test_group,
            &Some("127.0.0.2".to_owned()),
            None
        ));
    }

    #[derive(Default)]
    struct Sink(std::sync::Arc<std::sync::Mutex<Vec<String>>>);
    impl Actor for Sink {
        type Context = Context<Self>;
    }
    impl Handler<TextMessage> for Sink {
        type Result = ();
        fn handle(&mut self, msg: TextMessage, _: &mut Context<Self>) {
            self.0.lock().unwrap().push(msg.1);
        }
    }

    fn test_server() -> actix::Addr<ChannelServer> {
        test_server_with(Settings::default())
    }

    fn test_server_with(settings: Settings) -> actix::Addr<ChannelServer> {
        let metrics = Arc::new(StatsdClient::builder("test", cadence::NopMetricSink).build());
        ChannelServer::new(&settings, &MozLogger::default(), metrics).start()
    }

    fn connect(
        channel: ChannelID,
        initial_connect: bool,
        resume: Option<String>,
        sink: &Sink,
    ) -> Connect {
        Connect {
            addr: Sink(sink.0.clone()).start().recipient(),
            channel,
            remote: None,
            initial_connect,
            resume,
            resumable: true,
        }
    }

    /// Joins a second participant and returns its session id and resume token.
    async fn join(server: &actix::Addr<ChannelServer>, channel: ChannelID) -> (usize, String) {
        server
            .send(connect(channel, true, None, &Sink::default()))
            .await
            .unwrap();
        let sink = Sink::default();
        let id = server
            .send(connect(channel, false, None, &sink))
            .await
            .unwrap();
        actix_rt::task::yield_now().await;
        let greeting: serde_json::Value = serde_json::from_str(&sink.0.lock().unwrap()[0]).unwrap();
        (id, greeting["resume"].as_str().unwrap().to_owned())
    }

    #[actix_rt::test]
    async fn test_dropped_participant_can_resume() {
        let server = test_server();
        let channel = ChannelID::default();
        let (id, token) = join(&server, channel).await;
        server
            .send(Disconnect {
                channel,
                id,
                reason: DisconnectReason::None,
            })
            .await
            .unwrap();
        let resumed = server
            .send(connect(channel, false, Some(token), &Sink::default()))
            .await
            .unwrap();
        assert_ne!(resumed, 0);
    }

    #[actix_rt::test]
    async fn test_closed_participant_is_not_held() {
        let server = test_server();
        let channel = ChannelID::default();
        let (id, token) = join(&server, channel).await;
        server
            .send(Disconnect {
                channel,
                id,
                reason: DisconnectReason::Closed,
            })
            .await
            .unwrap();
        let resumed = server
            .send(connect(channel, false, Some(token), &Sink::default()))
            .await
            .unwrap();
        assert_eq!(resumed, 0);
    }

    #[actix_rt::test]
    async fn test_resume_after_window_is_refused() {
        // A heartbeat longer than the test keeps the sweep from running.
        let server = test_server_with(Settings {
            resume_window: 1,
            heartbeat_interval: 60,
            ..Settings::default()
        });
        let channel = ChannelID::default();
        let (id, token) = join(&server, channel).await;
        server
            .send(Disconnect {
                channel,
                id,
                reason: DisconnectReason::None,
            })
            .await
            .unwrap();
        actix_rt::time::sleep(Duration::from_millis(1100)).await;
        let resumed = server
            .send(connect(channel, false, Some(token), &Sink::default()))
            .await
            .unwrap();
        assert_eq!(resumed, 0);
    }

    #[actix_rt::test]
    async fn test_resume_with_previous_token_after_lost_greeting() {
        let server = test_server();
        let channel = ChannelID::default();
        let (id, token) = join(&server, channel).await;
        let drop = |id| Disconnect {
            channel,
            id,
            reason: DisconnectReason::None,
        };
        server.send(drop(id)).await.unwrap();
        let resumed = server
            .send(connect(
                channel,
                false,
                Some(token.clone()),
                &Sink::default(),
            ))
            .await
            .unwrap();
        server.send(drop(resumed)).await.unwrap();
        let again = server
            .send(connect(channel, false, Some(token), &Sink::default()))
            .await
            .unwrap();
        assert_ne!(again, 0);
    }

    #[actix_rt::test]
    async fn test_legacy_participant_is_not_held() {
        let server = test_server();
        let channel = ChannelID::default();
        let id = server
            .send(Connect {
                resumable: false,
                ..connect(channel, true, None, &Sink::default())
            })
            .await
            .unwrap();
        server
            .send(Disconnect {
                channel,
                id,
                reason: DisconnectReason::None,
            })
            .await
            .unwrap();
        assert!(server.send(ListChannels).await.unwrap().is_empty());
    }

    #[actix_rt::test]
    async fn test_resume_into_gone_channel() {
        let server = test_server();
        let session = server
            .send(connect(
                ChannelID::default(),
                false,
                Some("0".repeat(32)),
                &Sink::default(),
            ))
            .await
            .unwrap();
        assert_eq!(session, CHANNEL_GONE);
    }

    #[actix_rt::test]
    async fn test_no_resume_token_when_disabled() {
        let server = test_server_with(Settings {
            resume_window: 0,
            ..Settings::default()
        });
        let sink = Sink::default();
        server
            .send(connect(ChannelID::default(), true, None, &sink))
            .await
            .unwrap();
        actix_rt::task::yield_now().await;
        let greeting: serde_json::Value = serde_json::from_str(&sink.0.lock().unwrap()[0]).unwrap();
        assert!(greeting.get("resume").is_none());
    }

    #[actix_rt::test]
    async fn test_refused_initial_resume_leaves_no_channel() {
        let server = test_server();
        let session = server
            .send(connect(
                ChannelID::default(),
                true,
                Some("0".repeat(32)),
                &Sink::default(),
            ))
            .await
            .unwrap();
        assert_eq!(session, 0);
        assert!(server.send(ListChannels).await.unwrap().is_empty());
    }
}
