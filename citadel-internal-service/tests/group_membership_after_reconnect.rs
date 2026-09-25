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
    use crate::common::open_localhost_connection;
    use citadel_internal_service_types::{
        InternalServiceRequest, InternalServiceResponse, MessageGroupKey,
    };
    use std::error::Error;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use uuid::Uuid;

    type Service = (
        UnboundedSender<InternalServiceRequest>,
        UnboundedReceiver<InternalServiceResponse>,
    );

    /// Ends the member's session, as a user signing out does.
    async fn sign_out(
        tx: &UnboundedSender<InternalServiceRequest>,
        rx: &mut UnboundedReceiver<InternalServiceResponse>,
        cid: u64,
    ) -> Result<(), Box<dyn Error>> {
        tx.send(InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid,
        })?;
        let ended = recv_until(rx, "Disconnect answer", |r| {
            matches!(
                r,
                InternalServiceResponse::DisconnectNotification(_)
                    | InternalServiceResponse::DisconnectFailure(_)
            )
        })
        .await;
        assert!(
            matches!(ended, InternalServiceResponse::DisconnectNotification(_)),
            "the member could not end its session: {ended:?}"
        );
        Ok(())
    }

    /// Signs `{tag}.1` in again on a new localhost connection.
    async fn sign_in_again(
        service_addr: SocketAddr,
        tag: &str,
        member_cid: u64,
    ) -> Result<Service, Box<dyn Error>> {
        let (new_tx, mut new_rx) = open_localhost_connection(service_addr).await?;
        let connect_id = Uuid::new_v4();
        new_tx.send(InternalServiceRequest::Connect {
            username: format!("{tag}.1"),
            password: b"secret_1".to_vec().into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            request_id: connect_id,
            server_password: None,
        })?;
        let connected = recv_until(&mut new_rx, "Connect answer", |r| match r {
            InternalServiceResponse::ConnectSuccess(s) => s.request_id == Some(connect_id),
            InternalServiceResponse::ConnectFailure(f) => f.request_id == Some(connect_id),
            InternalServiceResponse::SessionAlreadyActive(a) => a.request_id == Some(connect_id),
            _ => false,
        })
        .await;
        let InternalServiceResponse::ConnectSuccess(success) = connected else {
            panic!("the member could not sign in again: {connected:?}");
        };
        assert_eq!(success.cid, member_cid, "a CID is permanent per account");
        Ok((new_tx, new_rx))
    }

    fn rejoined(r: &InternalServiceResponse, member_cid: u64, group_key: MessageGroupKey) -> bool {
        matches!(
            r,
            InternalServiceResponse::GroupChannelCreateSuccess(s)
                if s.cid == member_cid && s.group_key == group_key && s.request_id.is_none()
        )
    }

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
        let (_new_tx, mut new_rx) = sign_in_again(service_addr, tag, member_cid).await?;

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

        let (_new_tx, mut new_rx) = sign_in_again(service_addr, tag, member_cid).await?;
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
