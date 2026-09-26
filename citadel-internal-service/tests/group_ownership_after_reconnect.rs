//! A group OWNER whose session is re-established keeps its group, with no re-invite.
//!
//! The server used to delete an owner's groups the moment its session ended, so a server
//! deploy, a network drop or a sign-out destroyed every group it held, for every member.
//! The SDK now holds a departed owner's (flat) groups for a grace period and, when the
//! owner reconnects, sends it `RestoreOwnership`: the owner re-founds the tree with fresh
//! secrets and the SDK opens a group channel for it unasked, which the agent must adopt
//! into the session as it does a channel the session asked for. The server then prompts
//! the members to rejoin; a member that already has a channel keeps it.
//!
//! Two ways the owner's session comes back: the agent's own reconnect after the server
//! link is cut (the entry stays in the map, marked reconnecting), and the user signing
//! out and in again (the entry is removed and a new one inserted).

#[allow(dead_code)]
#[path = "reconnect_support/mod.rs"]
mod reconnect;
#[allow(dead_code)]
#[path = "server_host_support/mod.rs"]
mod support;

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        group_message_arrives, joined_group_on_one_service, joined_group_on_one_service_reaching,
        recv_until, send_group_message, OneServiceGroup,
    };
    use crate::common::group_rejoin::{rejoined, send_until_received, sign_in_again, sign_out};
    use crate::reconnect::Proxy;
    use citadel_sdk::prelude::StackedRatchet;
    use std::error::Error;

    #[tokio::test]
    async fn an_owner_whose_server_link_is_cut_keeps_its_group() -> Result<(), Box<dyn Error>> {
        let (server, server_addr) =
            crate::common::server_info_skip_cert_verification::<StackedRatchet>();
        drop(tokio::spawn(server));
        // Only the owner goes through the proxy, so only its link is cut.
        let proxy = Proxy::start(server_addr).await?;
        let OneServiceGroup {
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
            ..
        } = joined_group_on_one_service_reaching("groupownerlink", [proxy.addr, server_addr])
            .await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before");
        group_message_arrives(&mut member_rx, b"before")
            .await
            .ok_or("control: no delivery before the link was cut")?;

        proxy.sever();

        // The re-founded group's channel, unasked. It can precede ServerReconnected, so
        // nothing else is waited for first.
        let _ = recv_until(&mut owner_rx, "the owner's re-founded channel", |r| {
            rejoined(r, owner_cid, group_key)
        })
        .await;

        let heard = send_until_received(&owner_tx, owner_cid, group_key, &mut member_rx, "after")
            .await
            .ok_or("the member never heard the owner after the owner's reconnect")?;
        assert_eq!(heard.peer_cid, owner_cid);
        let replied =
            send_until_received(&member_tx, member_cid, group_key, &mut owner_rx, "reply")
                .await
                .ok_or("the owner's re-founded group never delivered the member's reply")?;
        assert_eq!(replied.peer_cid, member_cid);
        Ok(())
    }

    #[tokio::test]
    async fn an_owner_who_signs_out_and_in_keeps_its_group() -> Result<(), Box<dyn Error>> {
        let tag = "groupownersignin";
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service(tag).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"before");
        group_message_arrives(&mut member_rx, b"before")
            .await
            .ok_or("control: no delivery before the owner signed out")?;

        sign_out(&owner_tx, &mut owner_rx, owner_cid).await?;
        let (new_tx, mut new_rx) =
            sign_in_again(service_addr, &format!("{tag}.0"), b"secret_0", owner_cid).await?;
        let _ = recv_until(&mut new_rx, "the owner's re-founded channel", |r| {
            rejoined(r, owner_cid, group_key)
        })
        .await;

        let heard = send_until_received(&new_tx, owner_cid, group_key, &mut member_rx, "after")
            .await
            .ok_or("the member never heard the owner after it signed in again")?;
        assert_eq!(heard.peer_cid, owner_cid);
        let replied = send_until_received(&member_tx, member_cid, group_key, &mut new_rx, "reply")
            .await
            .ok_or("the owner's re-founded group never delivered the member's reply")?;
        assert_eq!(replied.peer_cid, member_cid);
        Ok(())
    }
}
