//! A group member whose C2S session is re-established -- `Disconnect`, then a
//! fresh `Connect` on another localhost connection -- while the group lives on
//! at the server.
//!
//! Group messages are end-to-end encrypted under a CGKA state held by the SDK
//! session (`state_container.group_cgka`), so a new session starts with neither
//! that state nor a group channel. The server's record of membership outlives
//! the session, so it keeps relaying the group's ciphertext to it.
//!
//! This used to need the OWNER to re-invite the member. The SDK now prompts a
//! member it still lists to rejoin as soon as the member's connect is
//! acknowledged (`GroupBroadcast::RestoreMembership`); the member publishes a
//! fresh KeyPackage, the owner re-adds it, and the member's session gets an
//! unsolicited `GroupChannelCreated`, which the agent reports as
//! `GroupChannelCreateSuccess` with no `request_id`. Nobody calls anything
//! group-related.
//!
//! A message the new session receives before its key arrives is no longer
//! dropped in silence: the SDK reports `GroupBroadcast::MessageDropped`, and the
//! agent passes it to the UI as `GroupMessageDroppedNotification`.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        group_message_arrives, joined_group_on_one_service, recv_until, send_group_message,
        OneServiceGroup,
    };
    use crate::common::group_rejoin::{rejoined, sign_in_again, sign_out};
    use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
    use std::error::Error;
    use std::time::Duration;
    use uuid::Uuid;

    #[tokio::test]
    async fn a_member_rejoins_by_itself_after_its_session_is_reestablished(
    ) -> Result<(), Box<dyn Error>> {
        let tag = "groupreconnect";
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, _owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service(tag).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before");
        group_message_arrives(&mut member_rx, b"before")
            .await
            .ok_or("control: no delivery before the reconnect")?;

        sign_out(&member_tx, &mut member_rx, member_cid).await?;
        let (_new_tx, mut new_rx) =
            sign_in_again(service_addr, &format!("{tag}.1"), b"secret_1", member_cid).await?;

        // No re-invite, no accept: the channel must come back unasked.
        let _ = recv_until(&mut new_rx, "unsolicited GroupChannelCreateSuccess", |r| {
            rejoined(r, member_cid, group_key)
        })
        .await;

        send_group_message(&owner_tx, owner_cid, group_key, b"after the rejoin");
        let after = group_message_arrives(&mut new_rx, b"after the rejoin")
            .await
            .ok_or("the member's new session rejoined but never received the group")?;
        assert_eq!(after.cid, member_cid);
        assert_eq!(after.peer_cid, owner_cid);
        Ok(())
    }

    #[tokio::test]
    async fn a_message_the_new_session_cannot_read_reaches_the_ui_as_dropped(
    ) -> Result<(), Box<dyn Error>> {
        let tag = "groupdropped";
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, _owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service(tag).await?;

        sign_out(&member_tx, &mut member_rx, member_cid).await?;

        // The owner keeps talking through the member's sign-in. Whatever the relay
        // hands the new session before the owner has re-added it is ciphertext
        // under a key that session does not hold.
        let flood_tx = owner_tx.clone();
        let flood = tokio::spawn(async move {
            for n in 0u32.. {
                if flood_tx
                    .send(InternalServiceRequest::GroupMessage {
                        cid: owner_cid,
                        message: format!("flood-{n}").into_bytes(),
                        group_key,
                        request_id: Uuid::new_v4(),
                    })
                    .is_err()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let (_new_tx, mut new_rx) =
            sign_in_again(service_addr, &format!("{tag}.1"), b"secret_1", member_cid).await?;
        let dropped = recv_until(&mut new_rx, "GroupMessageDroppedNotification", |r| {
            matches!(
                r,
                InternalServiceResponse::GroupMessageDroppedNotification(_)
            )
        })
        .await;
        flood.abort();
        let InternalServiceResponse::GroupMessageDroppedNotification(dropped) = dropped else {
            unreachable!()
        };
        assert_eq!(
            dropped.cid, member_cid,
            "reported to the session it was for"
        );
        assert_eq!(dropped.group_key, group_key);
        assert_eq!(dropped.sender, owner_cid, "names who sent it");
        assert!(!dropped.reason.is_empty(), "says why");
        assert_eq!(dropped.request_id, None, "it answers no request");
        Ok(())
    }
}
