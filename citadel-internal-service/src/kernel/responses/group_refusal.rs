//! The SDK's `GroupBroadcast::MessageRefused`, for the sender's window: the server refused to
//! relay a message because it broke one of the group's caps, so it reached nobody. It reaches
//! the agent by the group channel or as a kernel `GroupEvent`, and both build it here; it is
//! never dropped silently.

use citadel_internal_service_types::{
    GroupMessageRefusalReason, GroupMessageRefusedNotification, InternalServiceResponse,
};
use citadel_sdk::prelude::{GroupMessageRefusal, MessageGroupKey};

pub(crate) fn message_refused(
    cid: u64,
    group_key: MessageGroupKey,
    refusal: GroupMessageRefusal,
) -> InternalServiceResponse {
    let refusal = match refusal {
        GroupMessageRefusal::TooLarge { bytes, max_bytes } => {
            GroupMessageRefusalReason::TooLarge { bytes, max_bytes }
        }
        GroupMessageRefusal::RateLimited { max_per_second } => {
            GroupMessageRefusalReason::RateLimited { max_per_second }
        }
    };
    InternalServiceResponse::GroupMessageRefusedNotification(GroupMessageRefusedNotification {
        cid,
        group_key,
        refusal,
        request_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: MessageGroupKey = MessageGroupKey { cid: 7, mgid: 3 };

    fn refused(refusal: GroupMessageRefusal) -> GroupMessageRefusedNotification {
        match message_refused(42, KEY, refusal) {
            InternalServiceResponse::GroupMessageRefusedNotification(n) => n,
            other => panic!("expected a refusal notification, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_names_the_sender_the_group_and_the_cap() {
        let n = refused(GroupMessageRefusal::TooLarge {
            bytes: 20_000,
            max_bytes: 16_384,
        });
        assert_eq!((n.cid, n.group_key, n.request_id), (42, KEY, None));
        assert_eq!(
            n.refusal,
            GroupMessageRefusalReason::TooLarge {
                bytes: 20_000,
                max_bytes: 16_384
            }
        );
        let n = refused(GroupMessageRefusal::RateLimited { max_per_second: 4 });
        assert_eq!(
            n.refusal,
            GroupMessageRefusalReason::RateLimited { max_per_second: 4 }
        );
    }
}
