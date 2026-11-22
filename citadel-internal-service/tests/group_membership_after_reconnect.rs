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
//! A member keeps its group state in its account store, so a returning session
//! resumes it (`ResumeMembership`) instead of waiting for the owner to re-add it:
//! the second test holds the owner away while the member returns, and a third
//! member's message must still be read.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        accept_invitation, create_inviting, group_message_arrives,
        joined_group_on_one_service_reaching, peered, recv_until, send_group_message,
        OneServiceGroup,
    };
    use crate::common::group_rejoin::{rejoined, sign_in_again, sign_out};
    use crate::common::{
        server_info_skip_cert_verification, sessions_on_one_service_reaching, setup_log,
    };
    use citadel_sdk::prelude::StackedRatchet;
    use std::error::Error;

    #[tokio::test]
    async fn a_member_rejoins_by_itself_after_its_session_is_reestablished(
    ) -> Result<(), Box<dyn Error>> {
        let tag = "groupreconnect";
        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        drop(tokio::spawn(server));
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, _owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service_reaching(tag, [server_addr; 2]).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before");
        group_message_arrives(&mut member_rx, b"before")
            .await
            .ok_or("control: no delivery before the reconnect")?;

        sign_out(&member_tx, &mut member_rx, member_cid).await?;
        let (_new_tx, mut new_rx) = sign_in_again(
            service_addr,
            server_addr,
            &format!("{tag}.1"),
            b"secret_1",
            member_cid,
        )
        .await?;

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
    async fn a_member_back_while_its_owner_is_away_reads_the_group() -> Result<(), Box<dyn Error>> {
        let tag = "groupresumed";
        setup_log();
        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        drop(tokio::spawn(server));
        let (service_addr, mut sessions) =
            sessions_on_one_service_reaching(tag, &[server_addr; 3]).await?;
        let mut talker = sessions.remove(2);
        let mut member = sessions.remove(1);
        let mut owner = sessions.remove(0);
        peered(&mut owner, &mut member).await?;
        peered(&mut owner, &mut talker).await?;
        let group_key =
            create_inviting(&owner.0, &mut owner.1, owner.2, &[member.2, talker.2]).await?;
        accept_invitation(&member.0, &mut member.1, member.2, group_key).await?;
        accept_invitation(&talker.0, &mut talker.1, talker.2, group_key).await?;

        // Only the owner can re-add a member that kept nothing. With it away, the member must
        // resume the group state it kept to read anything.
        sign_out(&owner.0, &mut owner.1, owner.2).await?;
        sign_out(&member.0, &mut member.1, member.2).await?;
        let (_new_tx, mut new_rx) = sign_in_again(
            service_addr,
            server_addr,
            &format!("{tag}.1"),
            b"secret_1",
            member.2,
        )
        .await?;
        let _ = recv_until(&mut new_rx, "the resumed group's channel", |r| {
            rejoined(r, member.2, group_key)
        })
        .await;

        send_group_message(&talker.0, talker.2, group_key, b"readable");
        let read = group_message_arrives(&mut new_rx, b"readable")
            .await
            .ok_or("the member resumed but could not read the group with its owner away")?;
        assert_eq!((read.cid, read.peer_cid), (member.2, talker.2));
        Ok(())
    }
}
