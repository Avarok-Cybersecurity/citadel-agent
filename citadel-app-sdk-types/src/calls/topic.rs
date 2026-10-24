//! `topic`: an app's named channels between the members who installed and allowed it. One Citadel
//! group per (install, topic), never a chat group; the server relays its messages without reading
//! them. Bulk data does not ride a topic: a topic announces, internal HTTP carries.

use super::Caller;
use crate::Bytes;
use alloc::string::String;
use alloc::vec::Vec;

/// A manifest names at most this many topics.
pub const MAX_TOPICS: usize = 4;
/// A topic name is 1 to this many of `[a-z0-9-]`.
pub const MAX_TOPIC_NAME_LEN: usize = 32;
/// The ceiling on a manifest's `limits.topics.max_msg_bytes`.
pub const TOPIC_MAX_MSG_BYTES_CEILING: u32 = 16_384;
/// The ceiling on a manifest's `limits.topics.max_msgs_per_s`.
pub const TOPIC_MAX_MSGS_PER_S_CEILING: u32 = 4;
/// The tag of the envelope a topic message travels in, `{t, name, body}` in CBOR: a group message
/// without it is not a topic message.
pub const TOPIC_ENVELOPE_TAG: &str = "citadel-topic/1";
/// The most bytes the envelope adds to a body within the ceiling: the 3-entry map head (1), the
/// keys `t`, `name`, `body` (2 + 5 + 5), the tag (1 + 15), a longest name (2 + 32) and a body's
/// byte-string head (3 from 256 bytes up). `max_msg_bytes` counts the body; this is the rest.
pub const TOPIC_ENVELOPE_OVERHEAD: u32 = 66;
/// `Refused::Limit` when a publish exceeds the install's message rate.
pub const LIMIT_TOPIC_RATE: &str = "topic_rate";
/// `Refused::Limit` when a message exceeds the install's `max_msg_bytes`.
pub const LIMIT_TOPIC_BYTES: &str = "topic_bytes";

/// Whether `name` is a topic name: 1-32 of `[a-z0-9-]`.
pub fn is_topic_name(name: &str) -> bool {
    (1..=MAX_TOPIC_NAME_LEN).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

abi_types! {
    /// Join topic `name` of this install. Reply `TopicOpened`.
    pub struct TopicOpen {
        pub name: String,
    }

    /// Who is on the topic now (usernames), as the host's group membership says.
    pub struct TopicOpened {
        pub name: String,
        pub members: Vec<String>,
    }

    /// One message to everyone on the topic. A refusal (`limit: topic_rate`, `topic_bytes`) is
    /// answered at once.
    pub struct TopicPublish {
        pub name: String,
        pub body: Bytes,
    }

    /// Leave the topic.
    pub struct TopicClose {
        pub name: String,
    }

    /// What happened on an open topic.
    #[serde(rename_all = "snake_case")]
    pub enum TopicEvent {
        /// `from.via` is `Group`; the host stamps it from the group, never from the body.
        Message { from: Caller, body: Bytes },
        /// The topic's membership changed: who is on it now.
        Members(Vec<String>),
        /// A message from `from` could not be read (an older epoch, or a sender the host does
        /// not admit).
        Dropped { from: String },
    }
}
