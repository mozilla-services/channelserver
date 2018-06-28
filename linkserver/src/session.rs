//! MessageRouter sends these messages to the websocket client
use uuid::Uuid;

#[derive(Message)]
pub struct Message(pub String);

#[derive(Message)]
pub struct DropConnection;

/// ClientSession actor for each websocket client.
pub struct ClientSession {
    /// Client id
    pub id: Option<(Uuid, usize)>,
}
