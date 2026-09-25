//! A peer that disconnected, and then declines every redial, stays disconnected
//! -- including when both sessions live on the same internal service.
//!
//! Seen live (the UI's "Pause connection", two accounts on one agent): lara
//! disconnected max and declined his redials, yet ~18 s later max was "Online",
//! his console said "Already connected to peer <lara>", and messages flowed
//! both ways.
//!
//! The obvious suspect was a one-sided disconnect. It is not: both orientations
//! below show the drop reaching the other session (it is told, and its send is
//! refused). What reconnects them is the REDIAL. Declining a PeerConnect sends
//! a PostConnect carrying `Decline`, and the SDK recorded every outgoing
//! PostConnect -- the refusal included -- in `outgoing_peer_connect_attempts`,
//! an entry only a created channel consumes. On the next redial the SDK's
//! client-side simultaneous-connect rule read that leftover as "we are both
//! dialling", and the lower CID auto-accepted at protocol level without asking
//! its kernel: `PeerConnectSuccess` on redial 1, with the acceptor never asked.
//!
//! Fixed in Citadel-Protocol (fix/a-decline-is-not-an-outgoing-attempt). Against
//! the SDK this repository currently locks, the redial loop below FAILS; it
//! passes once the lock includes that fix.
use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::group::recv_until;
    use crate::common::{setup_log, two_sessions_on_one_service};
    use citadel_internal_service_types::{
        DisconnectNotification, InternalServiceRequest, InternalServiceResponse, MessageSendFailure,
    };
    use citadel_sdk::logging::info;
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::time::Duration;
    use uuid::Uuid;

    /// A dialled B, and A disconnects.
    #[tokio::test]
    async fn the_initiators_disconnect_reaches_the_acceptor() -> Result<(), Box<dyn Error>> {
        disconnect_reaches_the_other_session("pause-i", true).await
    }

    /// B dialled A, and A -- the side that accepted -- disconnects. The live case:
    /// the higher CID initiates, and the lower one pressed Pause.
    #[tokio::test]
    async fn the_acceptors_disconnect_reaches_the_initiator() -> Result<(), Box<dyn Error>> {
        disconnect_reaches_the_other_session("pause-a", false).await
    }

    async fn disconnect_reaches_the_other_session(
        tag: &str,
        a_dials: bool,
    ) -> Result<(), Box<dyn Error>> {
        setup_log();
        let ((mut tx_a, mut rx_a, cid_a), (mut tx_b, mut rx_b, cid_b)) =
            two_sessions_on_one_service(tag).await?;

        crate::common::register_p2p(
            &mut tx_a,
            &mut rx_a,
            cid_a,
            &mut tx_b,
            &mut rx_b,
            cid_b,
            SessionSecuritySettings::default(),
            None::<PreSharedKey>,
        )
        .await?;
        if a_dials {
            crate::common::connect_p2p(
                &mut tx_a,
                &mut rx_a,
                cid_a,
                &mut tx_b,
                &mut rx_b,
                cid_b,
                SessionSecuritySettings::default(),
                None::<PreSharedKey>,
            )
            .await?;
        } else {
            crate::common::connect_p2p(
                &mut tx_b,
                &mut rx_b,
                cid_b,
                &mut tx_a,
                &mut rx_a,
                cid_a,
                SessionSecuritySettings::default(),
                None::<PreSharedKey>,
            )
            .await?;
        }

        let request_id: Uuid = Uuid::new_v4();
        tx_a.send(InternalServiceRequest::PeerDisconnect {
            request_id,
            cid: cid_a,
            peer_cid: cid_b,
        })?;
        recv_until(&mut rx_a, "A's own disconnect answered", |r| {
            matches!(r, InternalServiceResponse::DisconnectNotification(n) if n.request_id == Some(request_id))
        })
        .await;

        // B is told, about ITS session: cid is B, peer_cid is A.
        let told: InternalServiceResponse =
            recv_until(&mut rx_b, "B hears that A disconnected", |r| {
                matches!(r, InternalServiceResponse::DisconnectNotification(_))
            })
            .await;
        match told {
            InternalServiceResponse::DisconnectNotification(DisconnectNotification {
                cid,
                peer_cid,
                ..
            }) => {
                assert_eq!(cid, cid_b, "addressed to B's session");
                assert_eq!(peer_cid, Some(cid_a), "naming A as the peer that left");
            }
            other => panic!("unexpected {other:?}"),
        }

        // B's auto-connect redials; A (paused) declines every time it is asked.
        // Each attempt must fail, and none may find the channel still up.
        for attempt in 0..3 {
            let dial_id: Uuid = Uuid::new_v4();
            tx_b.send(InternalServiceRequest::PeerConnect {
                request_id: dial_id,
                cid: cid_b,
                peer_cid: cid_a,
                udp_mode: Default::default(),
                session_security_settings: SessionSecuritySettings::default(),
                peer_session_password: None,
                turn: None,
            })?;
            // A declines if asked; B's answer may also come without A being asked.
            let dialled: InternalServiceResponse = tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    tokio::select! {
                        Some(r) = rx_a.recv() => {
                            if let InternalServiceResponse::PeerConnectNotification(n) = &r {
                                info!(target: "citadel", "[test] redial {attempt}: A asked, declining");
                                tx_a.send(InternalServiceRequest::PeerConnectAccept {
                                    request_id: Uuid::new_v4(),
                                    cid: cid_a,
                                    peer_cid: n.peer_cid,
                                    accept: false,
                                    udp_mode: Default::default(),
                                    session_security_settings: SessionSecuritySettings::default(),
                                    peer_session_password: None,
                                    turn: None,
                                }).expect("A's service is up");
                            } else {
                                info!(target: "citadel", "[test] A got {r:?}");
                            }
                        }
                        Some(r) = rx_b.recv() => match &r {
                            InternalServiceResponse::PeerConnectFailure(f) if f.request_id == Some(dial_id) => return r,
                            InternalServiceResponse::PeerConnectSuccess(s) if s.request_id == Some(dial_id) => return r,
                            other => info!(target: "citadel", "[test] B got {other:?}"),
                        }
                    }
                }
            })
            .await
            .expect("B's redial was never answered");
            info!(target: "citadel", "[test] redial {attempt}: B got {dialled:?}");
            tokio::time::sleep(Duration::from_millis(1000 << attempt)).await;
            match dialled {
                InternalServiceResponse::PeerConnectFailure(f) => assert!(
                    !f.message.contains("Already connected"),
                    "redial {attempt}: B still held the channel A dropped: {}",
                    f.message
                ),
                other => panic!("redial {attempt}: a declined dial connected anyway: {other:?}"),
            }
        }

        // And B no longer holds a sink to A: a send is refused, not delivered.
        let send_id: Uuid = Uuid::new_v4();
        tx_b.send(InternalServiceRequest::Message {
            request_id: send_id,
            message: b"while paused".to_vec(),
            cid: cid_b,
            peer_cid: Some(cid_a),
            security_level: SecurityLevel::Standard,
        })?;
        let answer: InternalServiceResponse =
            recv_until(&mut rx_b, "B's send answered", |r| match r {
                InternalServiceResponse::MessageSendFailure(f) => f.request_id == Some(send_id),
                InternalServiceResponse::MessageSendSuccess(s) => s.request_id == Some(send_id),
                _ => false,
            })
            .await;
        assert!(
            matches!(
                answer,
                InternalServiceResponse::MessageSendFailure(MessageSendFailure { .. })
            ),
            "B still had a channel to A after A disconnected: {answer:?}"
        );

        // Nothing from B reaches A.
        let leaked: Result<InternalServiceResponse, tokio::time::error::Elapsed> =
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let r = rx_a.recv().await.expect("service stream ended");
                    if matches!(r, InternalServiceResponse::MessageNotification(_)) {
                        return r;
                    }
                }
            })
            .await;
        assert!(
            leaked.is_err(),
            "A received B's message after disconnecting: {leaked:?}"
        );
        Ok(())
    }
}
