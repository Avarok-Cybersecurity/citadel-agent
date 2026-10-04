//! A window's answer to a `SecurityKeyChallengeNotification` (kernel/sign_in/key_relay.rs).

use crate::kernel::requests::HandledRequestResult;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, SecurityKeyAnswerFailure,
    SecurityKeyAnswerSuccess,
};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

pub async fn handle<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    uuid: Uuid,
    request: InternalServiceRequest,
) -> Option<HandledRequestResult> {
    let (request_id, challenge_id, outcome) = match request {
        InternalServiceRequest::SecurityKeyAnswer {
            request_id,
            challenge_id,
            credential_id,
            prf_output,
        } => {
            let outcome =
                this.key_challenges
                    .answer(uuid, challenge_id, credential_id, &prf_output);
            (request_id, challenge_id, outcome)
        }
        InternalServiceRequest::SecurityKeyDecline {
            request_id,
            challenge_id,
            reason,
        } => {
            let outcome = this.key_challenges.decline(uuid, challenge_id, reason);
            (request_id, challenge_id, outcome)
        }
        _ => unreachable!("Should never happen if programmed properly"),
    };
    let response = match outcome {
        Ok(cid) => InternalServiceResponse::SecurityKeyAnswerSuccess(SecurityKeyAnswerSuccess {
            cid,
            request_id: Some(request_id),
            challenge_id,
        }),
        Err(refused) => {
            warn!(target: "citadel", "[SignIn] Refused an answer to key challenge {challenge_id} from connection {uuid}: {}", refused.message);
            InternalServiceResponse::SecurityKeyAnswerFailure(SecurityKeyAnswerFailure {
                cid: refused.cid,
                request_id: Some(request_id),
                challenge_id,
                message: refused.message,
            })
        }
    };
    Some(HandledRequestResult { response, uuid })
}
