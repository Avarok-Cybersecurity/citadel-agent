//! `GroupCreateAppTopic`: the Citadel group one app topic rides (one per install and topic,
//! never a chat group). Joining, messaging, leaving and ending use the ordinary group requests;
//! only creation differs, and only in [`app_topic_group_options`].

use super::create::create_with;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_app_sdk_types::calls::topic::{
    TOPIC_ENVELOPE_OVERHEAD, TOPIC_MAX_MSGS_PER_S_CEILING, TOPIC_MAX_MSG_BYTES_CEILING,
};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    GroupCreateFailure, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::{
    GroupHierarchyMode, GroupMessageLimits, GroupType, MessageGroupOptions, Ratchet,
};
use uuid::Uuid;

/// What a `Flat` group's application message adds to its plaintext on the way to the server,
/// which caps the bytes it relays: citadel_treekem's `AppCiphertext` in the SDK's fixed-int
/// bincode (epoch u64, sender u32, generation u32, the ciphertext's u64 length) and the
/// ChaCha20-Poly1305 tag (16).
pub const FLAT_CIPHERTEXT_OVERHEAD: u32 = 8 + 4 + 4 + 8 + 16;

/// The relayed bytes of a topic message whose body is `max_msg_bytes`: the manifest counts the
/// body, the server the ciphertext of its envelope.
pub fn group_message_cap(max_msg_bytes: u32) -> u32 {
    max_msg_bytes + TOPIC_ENVELOPE_OVERHEAD + FLAT_CIPHERTEXT_OVERHEAD
}

/// The options every app-topic group is created with, and the one place they are chosen.
///
/// - `Private`: a join request is relayed to the owner, whose host accepts it only when the
///   kernel's `AuthorizePeer { direction: Topic }` admits the requester. A `Public` group
///   auto-accepts any requester in the protocol (`peer_layer.rs` `request_join`), bypassing the
///   kernel's audience, consent and block list.
/// - `Flat`: every member reads every message; a topic has no command hierarchy.
/// - No initial invitees: inviting (`GroupBroadcast::Add`) needs mutually registered peers, and
///   topic members need not be.
/// - `ephemeral`: the server relays to members online at the time and mails nobody, since a
///   topic's messages (heartbeats, announcements) are stale by the time an offline member reads.
/// - `limits`: the install's manifest `limits.topics`, which the server enforces per group, so a
///   host that does not hold to them is held to them anyway; the byte cap is the body's
///   ([`group_message_cap`]). Refused at zero or above the SDK ceilings.
pub fn app_topic_group_options(
    max_msg_bytes: u32,
    max_msgs_per_s: u32,
) -> Result<MessageGroupOptions, String> {
    if max_msg_bytes == 0 || max_msg_bytes > TOPIC_MAX_MSG_BYTES_CEILING {
        return Err(format!(
            "a topic message is 1 to {TOPIC_MAX_MSG_BYTES_CEILING} bytes, not {max_msg_bytes}"
        ));
    }
    if max_msgs_per_s > TOPIC_MAX_MSGS_PER_S_CEILING {
        return Err(format!(
            "a topic carries at most {TOPIC_MAX_MSGS_PER_S_CEILING} messages a second, not {max_msgs_per_s}"
        ));
    }
    let limits = GroupMessageLimits::new(group_message_cap(max_msg_bytes), max_msgs_per_s)
        .map_err(|e| format!("topic limits: {e}"))?;
    Ok(MessageGroupOptions {
        group_type: GroupType::Private,
        id: Uuid::new_v4().as_u128(),
        hierarchy: GroupHierarchyMode::Flat,
        ephemeral: true,
        limits: Some(limits),
    })
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupCreateAppTopic {
        cid,
        request_id,
        max_msg_bytes,
        max_msgs_per_s,
    } = request
    else {
        unreachable!("dispatched on GroupCreateAppTopic")
    };
    let response = match app_topic_group_options(max_msg_bytes, max_msgs_per_s) {
        Ok(options) => create_with(this, cid, request_id, None, options).await,
        Err(message) => InternalServiceResponse::GroupCreateFailure(GroupCreateFailure {
            cid,
            message,
            request_id: Some(request_id),
        }),
    };
    Some(HandledRequestResult { response, uuid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_topic_group_is_owner_accepted_flat_and_fresh() {
        let (a, b) = (
            app_topic_group_options(1024, 2).unwrap(),
            app_topic_group_options(1024, 2).unwrap(),
        );
        assert_eq!(a.group_type, GroupType::Private);
        assert_eq!(a.hierarchy, GroupHierarchyMode::Flat);
        assert_ne!(a.id, b.id, "each topic its own group");
    }

    #[test]
    fn an_app_topic_group_is_ephemeral_with_the_manifests_caps() {
        let options = app_topic_group_options(1024, 2).unwrap();
        assert!(options.ephemeral, "offline members are mailed nothing");
        let limits = options.limits.expect("the server enforces the caps");
        assert_eq!(limits.max_message_bytes(), group_message_cap(1024));
        assert_eq!(limits.max_messages_per_second(), 2);
    }

    /// A body of exactly `max_msg_bytes`, in the longest envelope, sealed as a `Flat` group seals
    /// it, is within the group's cap: what the host admits, the server relays. Measured on the
    /// SDK's own group state and serialiser, so a change to the ciphertext's layout shows here.
    #[test]
    fn a_full_body_in_the_longest_envelope_is_within_the_groups_cap() {
        use citadel_proto::prelude::SyncIO;
        use citadel_treekem::{GroupState, KeyPackage};
        let secret = [7u8; 32];
        let founder = KeyPackage::generate(1, &secret).unwrap().leaf;
        let mut group = GroupState::create(founder, secret);
        for max_msg_bytes in [1, 1024, TOPIC_MAX_MSG_BYTES_CEILING] {
            let cap = app_topic_group_options(max_msg_bytes, 2)
                .unwrap()
                .limits
                .unwrap()
                .max_message_bytes();
            let envelope = vec![0u8; (max_msg_bytes + TOPIC_ENVELOPE_OVERHEAD) as usize];
            let sealed = group.encrypt_message(&envelope).unwrap();
            let relayed = sealed.serialize_to_vector().unwrap().len();
            assert_eq!(relayed, cap as usize, "the cap is exactly a full message");
        }
    }

    #[test]
    fn caps_past_the_ceilings_or_at_zero_are_refused() {
        let bytes = TOPIC_MAX_MSG_BYTES_CEILING;
        let rate = TOPIC_MAX_MSGS_PER_S_CEILING;
        assert!(app_topic_group_options(bytes, rate).is_ok());
        assert!(app_topic_group_options(bytes + 1, rate).is_err());
        assert!(app_topic_group_options(bytes, rate + 1).is_err());
        assert!(app_topic_group_options(0, rate).is_err());
        assert!(app_topic_group_options(bytes, 0).is_err());
    }
}
