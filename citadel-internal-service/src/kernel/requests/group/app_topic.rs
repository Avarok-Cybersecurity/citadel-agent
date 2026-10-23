//! `GroupCreateAppTopic`: the Citadel group one app topic rides (one per install and topic,
//! never a chat group). Joining, messaging, leaving and ending use the ordinary group requests;
//! only creation differs, and only in [`app_topic_group_options`].

use super::create::create_with;
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::prelude::{GroupHierarchyMode, GroupType, MessageGroupOptions, Ratchet};
use uuid::Uuid;

/// The options every app-topic group is created with, and the one place they are chosen.
///
/// - `Private`: a join request is relayed to the owner, whose host accepts it only when the
///   kernel's `AuthorizePeer { direction: Topic }` admits the requester. A `Public` group
///   auto-accepts any requester in the protocol (`peer_layer.rs` `request_join`), bypassing the
///   kernel's audience, consent and block list.
/// - `Flat`: every member reads every message; a topic has no command hierarchy.
/// - No initial invitees: inviting (`GroupBroadcast::Add`) needs mutually registered peers, and
///   topic members need not be.
///
/// Protocol change P1 adds `ephemeral` (`mail_if_offline = false`, so heartbeats are not
/// mailboxed for offline members) and a server-side per-group rate and size cap. When the
/// pinned protocol carries them, they are set here and nowhere else.
pub fn app_topic_group_options() -> MessageGroupOptions {
    MessageGroupOptions {
        group_type: GroupType::Private,
        id: Uuid::new_v4().as_u128(),
        hierarchy: GroupHierarchyMode::Flat,
    }
}

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::GroupCreateAppTopic { cid, request_id } = request else {
        unreachable!("dispatched on GroupCreateAppTopic")
    };
    let response = create_with(this, cid, request_id, None, app_topic_group_options()).await;
    Some(HandledRequestResult { response, uuid })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_topic_group_is_owner_accepted_flat_and_fresh() {
        let (a, b) = (app_topic_group_options(), app_topic_group_options());
        assert_eq!(a.group_type, GroupType::Private);
        assert_eq!(a.hierarchy, GroupHierarchyMode::Flat);
        assert_ne!(a.id, b.id, "each topic its own group");
    }
}
