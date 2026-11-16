//! After `ConnectIdentity`'s sign-in: the challenge over the session's own link, and what a
//! failed one does (disconnect, warn the window, touch nothing).

use crate::kernel::identity::challenge::{judge, Mismatch};
use crate::kernel::identity::pin::Pin;
use crate::kernel::identity::IdentityHost;
use crate::kernel::server_link::{ServerLinks, SessionLink};
use crate::kernel::session_route::deliver;
use crate::kernel::CitadelWorkspaceService;
use citadel_identity::{MlDsa65Pk, NONCE_LEN};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, ServerIdentityMismatch,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// A fresh nonce over the session's own link, and the pinned server's answer to it.
pub(crate) async fn challenge<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    host: &IdentityHost,
    pin: &Pin,
    cid: u64,
) -> Result<MlDsa65Pk, Mismatch> {
    let failed = |why: String| Mismatch {
        pinned_fp: pin.server_fp.clone(),
        offered_fp: String::new(),
        why,
    };
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|err| failed(format!("no OS randomness: {err}")))?;
    let answer = host.waiters.expect(cid);
    let frame = host.waiters.codec().challenge(&nonce).map_err(failed)?;
    let link = ServerLinks::new(this.server_connection_map.clone());
    if let Err(err) = link.send_to_server(cid, frame).await {
        host.waiters.forget(cid);
        return Err(failed(format!("the challenge was not sent: {err}")));
    }
    let answer = match tokio::time::timeout(host.challenge_timeout, answer).await {
        Ok(Ok(answer)) => answer,
        Ok(Err(_)) => {
            return Err(failed(
                "the session closed before the server answered".into(),
            ))
        }
        Err(_) => {
            host.waiters.forget(cid);
            let why = format!(
                "the server did not answer within {:?}",
                host.challenge_timeout
            );
            return Err(failed(why));
        }
    };
    judge(pin, cid, &nonce, &answer)
}

/// Disconnects the session and warns its window. The account and its pin are not touched.
pub(crate) async fn refuse_server<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    cid: u64,
    pin: &Pin,
    mismatch: &Mismatch,
) {
    warn!(target: "citadel", "[ConnectIdentity] {cid}: {} failed its identity challenge ({}); disconnecting", pin.domain, mismatch.why);
    let disconnect = InternalServiceRequest::Disconnect {
        request_id: Uuid::new_v4(),
        cid,
    };
    if let Some(done) = crate::kernel::requests::disconnect::handle(this, uuid, disconnect).await {
        let _ = deliver(&this.tx_to_localhost_clients, &[done.uuid], done.response);
    }
    if let Some(host) = &this.identity {
        host.forget_session(cid);
    }
    let warning =
        InternalServiceResponse::ServerIdentityMismatchNotification(ServerIdentityMismatch {
            cid,
            request_id: None,
            domain: pin.domain.to_string(),
            pinned_fp: mismatch.pinned_fp.clone(),
            offered_fp: mismatch.offered_fp.clone(),
            message: mismatch.why.clone(),
        });
    let _ = deliver(&this.tx_to_localhost_clients, &[uuid], warning);
}
