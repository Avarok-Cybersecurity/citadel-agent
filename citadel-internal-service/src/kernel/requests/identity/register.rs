//! `RegisterIdentity` (identity doc §4.2 rev b, `server-identity-challenge.md` §3): decide the
//! username and the pin (kernel/identity/plan.rs), refuse a username this agent already holds,
//! register with the derived password through the ordinary Register path, pin, then sign in
//! with `ConnectIdentity`, which challenges the server.

use super::host;
use crate::kernel::identity::pin::{self, Pin};
use crate::kernel::identity::plan::{self, Plan, PlanError};
use crate::kernel::requests::HandledRequestResult;
use crate::kernel::session_route::deliver;
use crate::kernel::CitadelWorkspaceService;
use citadel_identity::WellKnownCitadel;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    FailureReason, IdentityDiscovery, InternalServiceRequest, InternalServiceResponse,
    RegisterFailure,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// `server_addr`'s verified `/.well-known/citadel` and the node it names.
pub(crate) async fn discover<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    server_addr: &str,
) -> Result<(WellKnownCitadel, String), PlanError> {
    let host = host(this).map_err(|(_, message)| PlanError::Invalid(message))?;
    let dialled = plan::dialled(server_addr).map_err(PlanError::Invalid)?;
    let document = host
        .well_known
        .fetch(&dialled)
        .await
        .map_err(PlanError::Invalid)?;
    plan::verified(&dialled, &document)
}

fn refused(
    uuid: Uuid,
    request_id: Uuid,
    reason: Option<FailureReason>,
    message: String,
    held_for_server: Option<String>,
) -> Option<HandledRequestResult> {
    let response = InternalServiceResponse::RegisterFailure(RegisterFailure {
        cid: 0,
        message,
        request_id: Some(request_id),
        reason_code: reason,
        admission_challenge: None,
        held_for_server,
    });
    Some(HandledRequestResult { response, uuid })
}

pub(crate) async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    command: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let InternalServiceRequest::RegisterIdentity {
        request_id,
        server_addr,
        handle,
        full_name,
        discovery,
        session_security_settings,
        server_password,
        admission_token,
    } = command
    else {
        unreachable!("dispatched by requests/mod.rs")
    };
    let refuse = |reason, message| refused(uuid, request_id, reason, message, None);
    let host = match host(this) {
        Ok(host) => host,
        Err((_, message)) => return refuse(None, message),
    };
    let Some(identity) = host.unlocked.current() else {
        let message = "Unlock your identity to add a server".to_string();
        return refuse(Some(FailureReason::IdentityLocked), message);
    };
    let dialled = match plan::dialled(&server_addr) {
        Ok(dialled) => dialled,
        Err(err) => return refuse(None, format!("{server_addr:?} is not a server name: {err}")),
    };
    let planned: Result<Plan, PlanError> = match &discovery {
        IdentityDiscovery::WellKnown => match host.well_known.fetch(&dialled).await {
            Ok(document) => plan::from_well_known(&handle, &dialled, &document),
            Err(err) => Err(PlanError::Invalid(err)),
        },
        IdentityDiscovery::Pinned { server_fp } => {
            plan::pinned(&handle, &server_addr, dialled, server_fp).map_err(PlanError::Invalid)
        }
    };
    let plan = match planned {
        Ok(plan) => plan,
        Err(err) => {
            let reason = match err {
                PlanError::NoCitadelEndpoint(_) => FailureReason::NoCitadelEndpoint,
                PlanError::Invalid(_) => FailureReason::ServerDiscoveryFailed,
            };
            return refuse(Some(reason), format!("Cannot add {server_addr}: {err}"));
        }
    };

    // Held anywhere, the same server included: a re-created server with a fresh database
    // would accept the name, and the SDK would save the new account over this one.
    let remote = this.remote();
    match crate::kernel::account_server::held(remote, &plan.username).await {
        Ok(None) => {}
        Ok(Some(server)) => {
            let held = server.as_requested();
            let message = format!(
                "This device already has an account named {} on {held}; forget it before registering again",
                plan.username
            );
            return refused(
                uuid,
                request_id,
                Some(FailureReason::UsernameHeld),
                message,
                Some(held),
            );
        }
        Err(err) => {
            return refuse(
                None,
                format!("could not check this device's accounts: {err}"),
            )
        }
    }

    let password = identity.derive_password(&plan.domain, &plan.username);
    let register = InternalServiceRequest::Register {
        request_id,
        server_addr: plan.dial.clone(),
        full_name,
        username: plan.username.clone(),
        proposed_password: password.as_str().as_bytes().to_vec().into(),
        connect_after_register: false,
        session_security_settings,
        server_password: server_password.clone(),
        admission_token,
    };
    let registered = crate::kernel::requests::register::handle(this, uuid, register).await?;
    let InternalServiceResponse::RegisterSuccess(mut success) = registered.response else {
        return Some(registered);
    };
    let cid = success.cid;
    let pinned = Pin {
        handle: plan.handle,
        domain: plan.domain,
        server_fp: plan.server_fp,
        server_pk: plan.server_pk,
        issued_at_s: plan.issued_at_s,
        id_fp: identity.id_fp(),
    };
    // The recovery phrase reaches the window whatever happens next: the server holds the account.
    let pin_stored = pin::store(remote, cid, &pinned).await;
    success.identity = pin_stored.is_ok().then(|| pinned.wire());
    if deliver(
        &this.tx_to_localhost_clients,
        &[uuid],
        InternalServiceResponse::RegisterSuccess(success),
    )
    .is_empty()
    {
        warn!(target: "citadel", "[RegisterIdentity] {cid} registered, but its window left before the recovery phrase reached it");
    }
    if let Err(err) = pin_stored {
        let message = format!(
            "Registered {}, but could not pin its server: {err}",
            plan.username
        );
        return refuse(None, message);
    }
    super::connect::connect_identity(
        this,
        uuid,
        request_id,
        cid,
        session_security_settings,
        server_password,
    )
    .await
}
