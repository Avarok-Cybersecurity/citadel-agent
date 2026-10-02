//! A two-member group, shared by the tests that need one.

use crate::{
    connect_p2p, get_free_port, register_and_connect_to_server_then_peers, register_p2p,
    two_sessions_on_one_service_at, two_sessions_on_one_service_reaching, PeerHandle,
    PeerServiceHandles,
};
use citadel_internal_service_types::{
    GroupCreateSuccess, GroupInviteNotification, GroupMessageNotification,
    GroupRespondRequestSuccess, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{
    MessageGroupKey, SessionSecuritySettingsBuilder, StackedRatchet, UserIdentifier,
};
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// Drains a service's response stream until `pred` matches, skipping the
/// unrelated notifications (member-state changes, channel-created acks)
/// that interleave nondeterministically with the responses under test.
pub async fn recv_until(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    what: &str,
    pred: impl Fn(&InternalServiceResponse) -> bool,
) -> InternalServiceResponse {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let response = rx.recv().await.expect("service stream ended");
            if pred(&response) {
                return response;
            }
            info!(target: "citadel", "[recv_until:{what}] skipping {response:?}");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

/// A live group with A as owner and B as an accepted member.
///
/// Extracted so the two departure routes -- B leaving, and the group
/// being ended out from under B -- are tested against the same starting
/// point rather than two hand-copied ones that could drift.
pub struct JoinedGroup {
    pub to_service_a: UnboundedSender<InternalServiceRequest>,
    pub from_service_a: UnboundedReceiver<InternalServiceResponse>,
    pub cid_a: u64,
    pub to_service_b: UnboundedSender<InternalServiceRequest>,
    pub from_service_b: UnboundedReceiver<InternalServiceResponse>,
    pub cid_b: u64,
    pub group_key: MessageGroupKey,
    /// B's service, so a test can open a second localhost connection to it.
    pub service_b_addr: SocketAddr,
}

pub async fn joined_group() -> Result<JoinedGroup, Box<dyn Error>> {
    crate::setup_log();
    let bind_address_internal_service_a: SocketAddr =
        format!("127.0.0.1:{}", get_free_port()).parse().unwrap();
    let bind_address_internal_service_b: SocketAddr =
        format!("127.0.0.1:{}", get_free_port()).parse().unwrap();

    let mut peer_return_handle_vec = register_and_connect_to_server_then_peers::<StackedRatchet>(
        vec![
            bind_address_internal_service_a,
            bind_address_internal_service_b,
        ],
        None,
        None,
    )
    .await?;

    let (to_service_a, mut from_service_a, cid_a) =
        peer_return_handle_vec.take_next_service_handle();
    let (to_service_b, mut from_service_b, cid_b) =
        peer_return_handle_vec.take_next_service_handle();

    let group_key = create_and_join(
        &to_service_a,
        &mut from_service_a,
        cid_a,
        &to_service_b,
        &mut from_service_b,
        cid_b,
    )
    .await?;
    Ok(JoinedGroup {
        to_service_a,
        from_service_a,
        cid_a,
        to_service_b,
        from_service_b,
        cid_b,
        group_key,
        service_b_addr: bind_address_internal_service_b,
    })
}

/// A creates a group inviting B, and B accepts. Returns the group's key.
pub async fn create_and_join(
    to_service_a: &UnboundedSender<InternalServiceRequest>,
    from_service_a: &mut UnboundedReceiver<InternalServiceResponse>,
    cid_a: u64,
    to_service_b: &UnboundedSender<InternalServiceRequest>,
    from_service_b: &mut UnboundedReceiver<InternalServiceResponse>,
    cid_b: u64,
) -> Result<MessageGroupKey, Box<dyn Error>> {
    let group_key = create_inviting(to_service_a, from_service_a, cid_a, &[cid_b]).await?;
    accept_invitation(to_service_b, from_service_b, cid_b, group_key).await?;
    Ok(group_key)
}

/// `cid` creates a group inviting each of `invitees`. Returns the group's key.
pub async fn create_inviting(
    tx: &UnboundedSender<InternalServiceRequest>,
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    invitees: &[u64],
) -> Result<MessageGroupKey, Box<dyn Error>> {
    tx.send(InternalServiceRequest::GroupCreate {
        cid,
        request_id: Uuid::new_v4(),
        initial_users_to_invite: Some(invitees.iter().copied().map(UserIdentifier::from).collect()),
    })?;
    let create_response = recv_until(rx, "GroupCreateSuccess", |r| {
        matches!(r, InternalServiceResponse::GroupCreateSuccess(..))
    })
    .await;
    let InternalServiceResponse::GroupCreateSuccess(GroupCreateSuccess { group_key, .. }) =
        create_response
    else {
        unreachable!()
    };
    Ok(group_key)
}

/// `cid` accepts its invitation to `group_key`.
pub async fn accept_invitation(
    tx: &UnboundedSender<InternalServiceRequest>,
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    cid: u64,
    group_key: MessageGroupKey,
) -> Result<(), Box<dyn Error>> {
    let invite = recv_until(rx, "GroupInviteNotification", |r| {
        matches!(r, InternalServiceResponse::GroupInviteNotification(..))
    })
    .await;
    let InternalServiceResponse::GroupInviteNotification(GroupInviteNotification {
        peer_cid,
        group_key: invited_key,
        ..
    }) = invite
    else {
        unreachable!()
    };
    assert_eq!(invited_key, group_key);
    tx.send(InternalServiceRequest::GroupRespondRequest {
        cid,
        peer_cid,
        group_key,
        response: true,
        request_id: Uuid::new_v4(),
        invitation: true,
    })?;
    let accept = recv_until(rx, "GroupRespondRequest response", |r| {
        matches!(
            r,
            InternalServiceResponse::GroupRespondRequestSuccess(..)
                | InternalServiceResponse::GroupRespondRequestFailure(..)
        )
    })
    .await;
    let InternalServiceResponse::GroupRespondRequestSuccess(GroupRespondRequestSuccess { .. }) =
        accept
    else {
        panic!("{cid} failed to accept the group invitation: {accept:?}")
    };
    Ok(())
}

/// Long enough for a broadcast through the server; a delivered message takes
/// well under a second.
pub const GROUP_MESSAGE_ARRIVES: Duration = Duration::from_secs(20);

/// Sends `text` to the group; the answer is left in the stream.
pub fn send_group_message(
    tx: &UnboundedSender<InternalServiceRequest>,
    cid: u64,
    group_key: MessageGroupKey,
    text: &[u8],
) {
    tx.send(InternalServiceRequest::GroupMessage {
        cid,
        message: text.to_vec(),
        group_key,
        request_id: Uuid::new_v4(),
    })
    .expect("service channel open");
}

/// The first `GroupMessageNotification` carrying `text`, or `None` within
/// `GROUP_MESSAGE_ARRIVES`.
pub async fn group_message_arrives(
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    text: &[u8],
) -> Option<GroupMessageNotification> {
    tokio::time::timeout(GROUP_MESSAGE_ARRIVES, async {
        loop {
            match rx.recv().await {
                Some(InternalServiceResponse::GroupMessageNotification(n))
                    if AsRef::<[u8]>::as_ref(&n.message) == text =>
                {
                    return n;
                }
                Some(_) => continue,
                None => std::future::pending::<()>().await,
            }
        }
    })
    .await
    .ok()
}

/// Owner and member as two sessions of ONE service, registered and connected
/// peers, the member in the owner's group. Session `i` is `{tag}.{i}` /
/// `secret_{i}`; the owner is `.0`.
pub struct OneServiceGroup {
    pub service_addr: SocketAddr,
    pub owner: PeerHandle,
    pub member: PeerHandle,
    pub group_key: MessageGroupKey,
}

pub async fn joined_group_on_one_service(tag: &str) -> Result<OneServiceGroup, Box<dyn Error>> {
    crate::setup_log();
    let sessions = two_sessions_on_one_service_at(tag).await?;
    join_the_second_to_the_firsts_group(sessions).await
}

/// As [`joined_group_on_one_service`], against a server the caller runs; the owner
/// reaches it at `server_addrs[0]`, the member at `server_addrs[1]`.
pub async fn joined_group_on_one_service_reaching(
    tag: &str,
    server_addrs: [SocketAddr; 2],
) -> Result<OneServiceGroup, Box<dyn Error>> {
    crate::setup_log();
    let sessions = two_sessions_on_one_service_reaching(tag, server_addrs).await?;
    join_the_second_to_the_firsts_group(sessions).await
}

async fn join_the_second_to_the_firsts_group(
    (service_addr, mut owner, mut member): (SocketAddr, PeerHandle, PeerHandle),
) -> Result<OneServiceGroup, Box<dyn Error>> {
    peered(&mut owner, &mut member).await?;
    let group_key = create_and_join(
        &owner.0,
        &mut owner.1,
        owner.2,
        &member.0,
        &mut member.1,
        member.2,
    )
    .await?;
    Ok(OneServiceGroup {
        service_addr,
        owner,
        member,
        group_key,
    })
}

/// Registers and connects `a` and `b` as peers, as a group invite between them needs.
pub async fn peered(a: &mut PeerHandle, b: &mut PeerHandle) -> Result<(), Box<dyn Error>> {
    let settings = SessionSecuritySettingsBuilder::default().build()?;
    register_p2p(
        &mut a.0, &mut a.1, a.2, &mut b.0, &mut b.1, b.2, settings, None,
    )
    .await?;
    connect_p2p(
        &mut a.0, &mut a.1, a.2, &mut b.0, &mut b.1, b.2, settings, None,
    )
    .await
}
