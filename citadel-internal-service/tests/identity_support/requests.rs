//! The identity requests as the tests send them, and the answers they wait for.

use super::{expect, ChallengeServer, Sink, Stream, UNLOCK};
use citadel_internal_service_test_common as common;
use citadel_internal_service_types::{
    ConnectSuccess, IdentityDiscovery, IdentityOutcome, InternalServiceRequest,
    InternalServiceResponse,
};
use std::error::Error;
use uuid::Uuid;

pub async fn create_identity(
    sink: &mut Sink,
    stream: &mut Stream,
) -> Result<String, Box<dyn Error>> {
    let request_id = Uuid::new_v4();
    common::send(
        sink,
        InternalServiceRequest::IdentityCreate {
            request_id,
            unlock_password: UNLOCK.as_bytes().to_vec().into(),
        },
    )
    .await?;
    expect(stream, |r| match r {
        InternalServiceResponse::IdentitySuccess(s) if s.request_id == Some(request_id) => {
            match s.outcome {
                IdentityOutcome::Created { id_fp, .. } => Some(Ok(id_fp)),
                other => Some(Err(format!("{other:?}"))),
            }
        }
        InternalServiceResponse::IdentityFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(f.message))
        }
        _ => None,
    })
    .await?
    .map_err(Into::into)
}

pub fn register_identity(
    request_id: Uuid,
    server: &ChallengeServer,
    handle: &str,
) -> InternalServiceRequest {
    InternalServiceRequest::RegisterIdentity {
        request_id,
        server_addr: server.addr.clone(),
        handle: handle.to_string(),
        full_name: "Identity Test".to_string(),
        discovery: IdentityDiscovery::Pinned {
            server_fp: server.fp(),
        },
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        server_password: None,
        admission_token: None,
    }
}

pub fn connect_identity(request_id: Uuid, cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::ConnectIdentity {
        request_id,
        cid,
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        server_password: None,
    }
}

/// The answer to `request_id`'s sign-in: the session's CID, or the failure.
pub async fn signed_in(
    stream: &mut Stream,
    request_id: Uuid,
) -> Result<Result<ConnectSuccess, String>, Box<dyn Error>> {
    expect(stream, |r| match r {
        InternalServiceResponse::ConnectSuccess(s) if s.request_id == Some(request_id) => {
            Some(Ok(s))
        }
        InternalServiceResponse::ConnectFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(format!("{:?}: {}", f.reason_code, f.message)))
        }
        InternalServiceResponse::RegisterFailure(f) if f.request_id == Some(request_id) => {
            Some(Err(format!("{:?}: {}", f.reason_code, f.message)))
        }
        _ => None,
    })
    .await
}

pub async fn disconnect(
    sink: &mut Sink,
    stream: &mut Stream,
    cid: u64,
) -> Result<(), Box<dyn Error>> {
    common::send(
        sink,
        InternalServiceRequest::Disconnect {
            request_id: Uuid::new_v4(),
            cid,
        },
    )
    .await?;
    expect(stream, |r| {
        matches!(r, InternalServiceResponse::DisconnectNotification(_)).then_some(())
    })
    .await
}
