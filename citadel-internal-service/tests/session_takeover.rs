//! Nobody may take a live session away from the connection that holds it.
//!
//! `ConnectionManagement` is exempt from the ownership gate in
//! `kernel/requests/mod.rs` — it carries its target inside `management_command`
//! rather than in a `cid` field, so `session_cid()` returns `None` for it and
//! the gate never runs. Every command inside it therefore acted on whatever CID
//! the caller named. A CID is a `u64` that travels in peer lists and
//! notifications; `GetSessions` will hand over every one of them.
//!
//! These tests assert the CONSEQUENCE, not the refusal message. A claim
//! re-points `associated_localhost_connection`, which is the field
//! `send_response_for_session` routes by — so the thing to prove is that the
//! victim's own stream still receives its session's notifications afterwards.
//! Asserting only "a failure came back" would pass against a handler that
//! refused and re-pointed anyway.

use citadel_internal_service_test_common as common;

#[cfg(test)]
mod tests {
    use crate::common::{two_sessions_on_one_service, PeerHandle};
    use citadel_internal_service_types::{
        ConfigCommand, InternalServiceRequest, InternalServiceResponse,
    };
    use citadel_sdk::prelude::*;
    use std::error::Error;
    use std::time::Duration;
    use uuid::Uuid;

    /// How long to wait for a notification that should arrive, and for one that
    /// should not. The second is a bound on patience, not on correctness: a
    /// too-short wait here would make the "victim still receives it" assertion
    /// flaky rather than wrong, and that assertion is the load-bearing one.
    const ARRIVES: Duration = Duration::from_secs(10);

    fn claim(session_cid: u64, only_if_orphaned: bool) -> InternalServiceRequest {
        InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::ClaimSession {
                session_cid,
                only_if_orphaned,
            },
        }
    }

    /// Read until a `ConnectionManagement*` response appears, so an unrelated
    /// notification already in the queue cannot be mistaken for the answer.
    async fn next_management_response(
        handle: &mut PeerHandle,
    ) -> Result<InternalServiceResponse, Box<dyn Error>> {
        let deadline = tokio::time::Instant::now() + ARRIVES;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let response = tokio::time::timeout(remaining, handle.1.recv())
                .await
                .map_err(|_| "no ConnectionManagement response before the deadline")?
                .ok_or("service closed the stream")?;
            if matches!(
                response,
                InternalServiceResponse::ConnectionManagementSuccess(_)
                    | InternalServiceResponse::ConnectionManagementFailure(_)
            ) {
                return Ok(response);
            }
        }
    }

    /// C1: `ClaimSession { only_if_orphaned: false }` on a live session held by
    /// another connection.
    ///
    /// `only_if_orphaned` is supplied by the caller, so it authorized nothing —
    /// `false` meant "take it from whoever has it". The victim got no signal.
    #[tokio::test]
    async fn a_live_session_cannot_be_claimed_by_another_connection() -> Result<(), Box<dyn Error>>
    {
        crate::common::setup_log();
        let (mut victim, mut thief) = two_sessions_on_one_service("takeover.claim").await?;
        let victim_cid = victim.2;

        thief.0.send(claim(victim_cid, false))?;
        match next_management_response(&mut thief).await? {
            InternalServiceResponse::ConnectionManagementFailure(failure) => {
                assert!(
                    failure.error.contains("in use by another connection"),
                    "refused, but for the wrong reason: {}",
                    failure.error
                );
            }
            other => panic!("the claim was not refused: {other:?}"),
        }

        // The consequence. A PeerRegister addressed to the victim's CID is
        // delivered through `associated_localhost_connection` — the exact field
        // a successful claim overwrites. If the claim had landed, this arrives
        // on the thief's stream and never on the victim's.
        thief.0.send(InternalServiceRequest::PeerRegister {
            request_id: Uuid::new_v4(),
            cid: thief.2,
            peer_cid: victim_cid,
            session_security_settings: SessionSecuritySettingsBuilder::default().build()?,
            connect_after_register: false,
            peer_session_password: None,
        })?;

        let notified = tokio::time::timeout(ARRIVES, async {
            loop {
                match victim.1.recv().await {
                    Some(InternalServiceResponse::PeerRegisterNotification(notification)) => {
                        return notification;
                    }
                    Some(_) => continue,
                    None => panic!("the victim's stream closed"),
                }
            }
        })
        .await
        .map_err(|_| "the victim never received its own session's notification")?;

        assert_eq!(
            notified.cid, victim_cid,
            "the notification reached the victim, but for the wrong session"
        );
        Ok(())
    }

    /// The claim path must still work for its legitimate case, or the fix above
    /// is indistinguishable from disabling the feature. A connection
    /// re-asserting a session it already holds is what
    /// `peer-registration-store/lifecycle.ts` does before every PeerRegister.
    #[tokio::test]
    async fn a_connection_may_still_claim_its_own_live_session() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let (mut owner, _other) = two_sessions_on_one_service("takeover.self").await?;
        let cid = owner.2;

        owner.0.send(claim(cid, false))?;
        match next_management_response(&mut owner).await? {
            InternalServiceResponse::ConnectionManagementSuccess(_) => Ok(()),
            other => panic!("a connection was refused its own session: {other:?}"),
        }
    }

    /// H1, first half: `DisconnectOrphan { session_cid: Some(cid) }` never
    /// checked that the target was orphaned. It removed any session named.
    #[tokio::test]
    async fn a_live_session_cannot_be_disconnected_by_another_connection(
    ) -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let (victim, mut thief) = two_sessions_on_one_service("takeover.disconnect").await?;
        let victim_cid = victim.2;

        thief.0.send(InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::DisconnectOrphan {
                session_cid: Some(victim_cid),
            },
        })?;
        match next_management_response(&mut thief).await? {
            InternalServiceResponse::ConnectionManagementFailure(failure) => {
                assert!(
                    failure.error.contains("in use by another connection"),
                    "refused, but for the wrong reason: {}",
                    failure.error
                );
            }
            other => panic!("the disconnect was not refused: {other:?}"),
        }

        // The consequence: the session is still there. A removal would make the
        // victim's own claim fail with "not found".
        let mut victim = victim;
        victim.0.send(claim(victim_cid, false))?;
        match next_management_response(&mut victim).await? {
            InternalServiceResponse::ConnectionManagementSuccess(_) => Ok(()),
            other => panic!("the victim's session did not survive: {other:?}"),
        }
    }

    /// `DisconnectOrphan` must disconnect, not merely forget.
    ///
    /// Removing the map entry is not a disconnect: nothing in `Connection` tears
    /// the SDK session down when it drops, and the C2S receive half is not even
    /// in it — it lives in the task the connect handler spawned and keeps
    /// running. So the handler answered "Disconnected orphan session X" while a
    /// `SessionState::Connected` session carried on with its keepalives.
    ///
    /// The consequence, which is what this asserts: the account was wedged until
    /// the process restarted. With the map entry gone, the next `Connect` skips
    /// the reuse path and calls `remote.connect()`, and the protocol refuses it
    /// with `SessionManagerSessionAlreadyExists`. `ClaimSession` and `Disconnect`
    /// both answer "not found", because the entry is gone. No wire command could
    /// reach the session that was still there.
    ///
    /// Asserting the reconnect rather than the success message is the point: a
    /// handler that removes and reports success passes any assertion about the
    /// message it just wrote.
    #[tokio::test]
    async fn a_disconnected_orphan_can_be_reconnected() -> Result<(), Box<dyn Error>> {
        crate::common::setup_log();
        let tag = "takeover.orphan_reconnect";
        let (mut owner, mut other) = two_sessions_on_one_service(tag).await?;
        let cid = owner.2;

        // Orphan it from the connection that holds it, which is allowed.
        owner.0.send(InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::ReleaseSession { session_cid: cid },
        })?;
        match next_management_response(&mut owner).await? {
            InternalServiceResponse::ConnectionManagementSuccess(_) => {}
            other => panic!("a connection could not release its own session: {other:?}"),
        }

        other.0.send(InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::DisconnectOrphan {
                session_cid: Some(cid),
            },
        })?;
        match next_management_response(&mut other).await? {
            InternalServiceResponse::ConnectionManagementSuccess(_) => {}
            resp => panic!("the orphan was not disconnected: {resp:?}"),
        }

        // The load-bearing assertion. If the protocol session survived, this
        // Connect is refused with SessionManagerSessionAlreadyExists.
        other.0.send(InternalServiceRequest::Connect {
            username: format!("{tag}.0"),
            password: b"secret_0".to_vec().into(),
            connect_mode: Default::default(),
            udp_mode: Default::default(),
            keep_alive_timeout: None,
            session_security_settings: Default::default(),
            request_id: Uuid::new_v4(),
            server_password: None,
        })?;

        let deadline = tokio::time::Instant::now() + ARRIVES;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let response = tokio::time::timeout(remaining, other.1.recv())
                .await
                .map_err(|_| "no Connect answer before the deadline")?
                .ok_or("service closed the stream")?;
            match response {
                InternalServiceResponse::ConnectSuccess(_) => return Ok(()),
                InternalServiceResponse::ConnectFailure(failure) => panic!(
                    "the account is wedged: the orphan was reported disconnected but the \
                     protocol session survived, so reconnecting is refused: {}",
                    failure.message
                ),
                _ => continue,
            }
        }
    }

    /// H1, second half: `ReleaseSession` stamped the nil UUID over any
    /// session's owner, marking somebody else's live session reclaimable.
    #[tokio::test]
    async fn a_live_session_cannot_be_released_by_another_connection() -> Result<(), Box<dyn Error>>
    {
        crate::common::setup_log();
        let (victim, mut thief) = two_sessions_on_one_service("takeover.release").await?;
        let victim_cid = victim.2;

        thief.0.send(InternalServiceRequest::ConnectionManagement {
            request_id: Uuid::new_v4(),
            management_command: ConfigCommand::ReleaseSession {
                session_cid: victim_cid,
            },
        })?;
        match next_management_response(&mut thief).await? {
            InternalServiceResponse::ConnectionManagementFailure(failure) => {
                assert!(
                    failure.error.contains("in use by another connection"),
                    "refused, but for the wrong reason: {}",
                    failure.error
                );
            }
            other => panic!("the release was not refused: {other:?}"),
        }

        // The consequence: the session is still HELD, not merely still present.
        // A successful release leaves it in the map but orphaned, so the thief
        // could then claim it with `only_if_orphaned: true` — the very check
        // the frontend relies on. That claim must still fail.
        thief.0.send(claim(victim_cid, true))?;
        match next_management_response(&mut thief).await? {
            InternalServiceResponse::ConnectionManagementFailure(_) => Ok(()),
            other => panic!("the session was released after all: {other:?}"),
        }
    }
}
