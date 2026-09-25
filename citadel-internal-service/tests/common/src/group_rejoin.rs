//! Signing a group member out and back in, and what it must see when it returns.

use crate::group::{recv_until, send_group_message, GROUP_MESSAGE_ARRIVES};
use citadel_internal_service_types::{
    GroupMessageNotification, InternalServiceRequest, InternalServiceResponse,
};
use citadel_sdk::prelude::MessageGroupKey;
use std::error::Error;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use uuid::Uuid;

/// Ends a session, as a user signing out does.
pub async fn sign_out(
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
        "could not end the session: {ended:?}"
    );
    Ok(())
}

/// Signs `username` in again on a new localhost connection; it must come back as `cid`.
pub async fn sign_in_again(
    service_addr: SocketAddr,
    username: &str,
    password: &[u8],
    cid: u64,
) -> Result<
    (
        UnboundedSender<InternalServiceRequest>,
        UnboundedReceiver<InternalServiceResponse>,
    ),
    Box<dyn Error>,
> {
    let (new_tx, mut new_rx) = crate::open_localhost_connection(service_addr).await?;
    let connect_id = Uuid::new_v4();
    new_tx.send(InternalServiceRequest::Connect {
        username: username.to_string(),
        password: password.to_vec().into(),
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
        panic!("could not sign in again: {connected:?}");
    };
    assert_eq!(success.cid, cid, "a CID is permanent per account");
    Ok((new_tx, new_rx))
}

/// The unsolicited `GroupChannelCreateSuccess` the SDK's rejoin or re-founding opens.
pub fn rejoined(r: &InternalServiceResponse, cid: u64, group_key: MessageGroupKey) -> bool {
    matches!(
        r,
        InternalServiceResponse::GroupChannelCreateSuccess(s)
            if s.cid == cid && s.group_key == group_key && s.request_id.is_none()
    )
}

/// Sends `{prefix}-N` every half second until one reaches `rx`, for up to
/// `GROUP_MESSAGE_ARRIVES`: after a re-founding, the member is moved to the new tree
/// only once its rejoin completes, so the first sends can precede it.
pub async fn send_until_received(
    tx: &UnboundedSender<InternalServiceRequest>,
    cid: u64,
    group_key: MessageGroupKey,
    rx: &mut UnboundedReceiver<InternalServiceResponse>,
    prefix: &str,
) -> Option<GroupMessageNotification> {
    let wanted = format!("{prefix}-");
    tokio::time::timeout(GROUP_MESSAGE_ARRIVES, async {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        let mut n = 0u32;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    send_group_message(tx, cid, group_key, format!("{wanted}{n}").as_bytes());
                    n += 1;
                }
                received = rx.recv() => match received {
                    Some(InternalServiceResponse::GroupMessageNotification(m))
                        if AsRef::<[u8]>::as_ref(&m.message).starts_with(wanted.as_bytes()) =>
                    {
                        return m;
                    }
                    Some(_) => {}
                    None => std::future::pending::<()>().await,
                },
            }
        }
    })
    .await
    .ok()
}
