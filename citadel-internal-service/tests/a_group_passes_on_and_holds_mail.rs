//! A group outlives its owner's departure and its members' absences, and the window is told.
//!
//! - When the owner leaves, the remaining member with the highest CID owns the group, every
//!   member's window gets `GroupOwnershipTransferredNotification` naming the new owner and the
//!   one that left, and the group is listed (`GroupListJoined`) under its new owner.
//! - A message sent while a member is signed out is held for it: the sender's delivery report
//!   lists the member as `stored`, and the member reads the message when it signs in again.
//!
//! Controls (each turns a test red): in kernel/responses/group_answers.rs, drop the
//! `OwnershipTransferred` arm (no window hears of the transfer); report every delivery as
//! `NotRelayed` (the sender's report lists no one as stored); in kernel/group_channels.rs, list
//! each group under `group_key.cid` (its creator, not its owner).

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::{
        accept_invitation, create_inviting, group_message_arrives,
        joined_group_on_one_service_reaching, peered, recv_until, send_group_message,
        OneServiceGroup,
    };
    use crate::common::group_rejoin::{send_until_held_for, sign_in_again, sign_out};
    use crate::common::{
        server_info_skip_cert_verification, sessions_on_one_service_reaching, setup_log,
    };
    use citadel_internal_service_types::{
        GroupMessageResponse, GroupOwnershipTransferredNotification, InternalServiceRequest,
        InternalServiceResponse, JoinedGroupOwner,
    };
    use citadel_sdk::prelude::{GroupDelivery, GroupStateSource, StackedRatchet};
    use std::error::Error;
    use uuid::Uuid;

    #[tokio::test]
    async fn the_highest_cid_member_owns_the_group_once_its_owner_leaves(
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        drop(tokio::spawn(server));
        let (_service, mut sessions) =
            sessions_on_one_service_reaching("groupsuccession", &[server_addr; 3]).await?;
        let mut second = sessions.remove(2);
        let mut first = sessions.remove(1);
        let mut owner = sessions.remove(0);
        peered(&mut owner, &mut first).await?;
        peered(&mut owner, &mut second).await?;
        let group_key =
            create_inviting(&owner.0, &mut owner.1, owner.2, &[first.2, second.2]).await?;
        accept_invitation(&first.0, &mut first.1, first.2, group_key).await?;
        accept_invitation(&second.0, &mut second.1, second.2, group_key).await?;

        owner.0.send(InternalServiceRequest::GroupLeave {
            cid: owner.2,
            group_key,
            request_id: Uuid::new_v4(),
        })?;

        let successor = first.2.max(second.2);
        for member in [&mut first, &mut second] {
            let told = recv_until(&mut member.1, "the ownership transfer", |r| {
                matches!(
                    r,
                    InternalServiceResponse::GroupOwnershipTransferredNotification(_)
                )
            })
            .await;
            let InternalServiceResponse::GroupOwnershipTransferredNotification(
                GroupOwnershipTransferredNotification {
                    cid,
                    group_key: key,
                    owner: new_owner,
                    previous,
                    request_id,
                },
            ) = told
            else {
                unreachable!()
            };
            assert_eq!((cid, key, request_id), (member.2, group_key, None));
            assert_eq!(new_owner, successor, "the highest remaining CID owns it");
            assert_eq!(previous, owner.2, "names the owner that left");

            let listed = Uuid::new_v4();
            member.0.send(InternalServiceRequest::GroupListJoined {
                cid: member.2,
                request_id: listed,
            })?;
            let listed = recv_until(&mut member.1, "the joined list", |r| {
                matches!(r, InternalServiceResponse::GroupListJoinedSuccess(s) if s.request_id == Some(listed))
            })
            .await;
            let InternalServiceResponse::GroupListJoinedSuccess(listed) = listed else {
                unreachable!()
            };
            assert_eq!(
                listed.owners,
                vec![JoinedGroupOwner {
                    group_key,
                    owner: successor
                }],
                "the group is listed under its new owner, not its creator"
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn a_member_away_has_its_mail_held_and_reads_it_on_return() -> Result<(), Box<dyn Error>>
    {
        let tag = "groupmail";
        let (server, server_addr) = server_info_skip_cert_verification::<StackedRatchet>();
        drop(tokio::spawn(server));
        let OneServiceGroup {
            service_addr,
            owner: (owner_tx, mut owner_rx, owner_cid),
            member: (member_tx, mut member_rx, member_cid),
            group_key,
        } = joined_group_on_one_service_reaching(tag, [server_addr; 2]).await?;

        send_group_message(&owner_tx, owner_cid, group_key, b"while here");
        group_message_arrives(&mut member_rx, b"while here")
            .await
            .ok_or("control: no delivery while the member is signed in")?;
        let here = delivery_report(&mut owner_rx).await;
        assert!(
            matches!(&here, GroupDelivery::Relayed { delivered, .. } if delivered == &[member_cid]),
            "{here:?}"
        );

        sign_out(&member_tx, &mut member_rx, member_cid).await?;
        // The report lists the member as `stored`: the helper returns only such a message.
        let held = send_until_held_for(
            &owner_tx,
            &mut owner_rx,
            owner_cid,
            group_key,
            member_cid,
            "away",
        )
        .await;

        let (_new_tx, mut new_rx) = sign_in_again(
            service_addr,
            server_addr,
            &format!("{tag}.1"),
            b"secret_1",
            member_cid,
        )
        .await?;
        let held = group_message_arrives(&mut new_rx, &held)
            .await
            .ok_or("the member signed in again but never read what was held for it")?;
        assert_eq!((held.cid, held.peer_cid), (member_cid, owner_cid));

        // It resumed the state it kept here, which the window need not announce.
        let restored = recv_until(&mut new_rx, "the restore notice", |r| {
            matches!(r, InternalServiceResponse::GroupRestoredNotification(n) if n.group_key == group_key)
        })
        .await;
        let InternalServiceResponse::GroupRestoredNotification(restored) = restored else {
            unreachable!()
        };
        assert_eq!(restored.source, GroupStateSource::ThisDevice);
        Ok(())
    }

    async fn delivery_report(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<InternalServiceResponse>,
    ) -> GroupDelivery {
        let report = recv_until(rx, "the delivery report", |r| {
            matches!(r, InternalServiceResponse::GroupMessageResponse(_))
        })
        .await;
        let InternalServiceResponse::GroupMessageResponse(GroupMessageResponse {
            delivery, ..
        }) = report
        else {
            unreachable!()
        };
        delivery
    }
}
