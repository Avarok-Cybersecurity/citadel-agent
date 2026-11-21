//! Through the real dispatcher (P12, P14): the origin a proof names is the one recorded for
//! the connection's handshake, whatever the request body says, and an issuer-only server is
//! refused `NoCitadelEndpoint` for both `ServerDiscovery` and `RegisterIdentity`.
//!
//! Control: in `lifecycle`, sign `Some(format!("https://{issuer}"))` (a value the page
//! controls) instead of `this.control_origins.origin(&uuid)` →
//! `a_page_supplied_origin_is_ignored` goes red: the foreign connection gets a proof the
//! issuer accepts.

use crate::kernel::identity::test_ports::{
    config, ManualClock, NoFetch, ServeDocument, SHIPPED_SIGN_LIMITS,
};
use crate::kernel::identity::WellKnownFetch;
use crate::kernel::requests::handle_request;
use crate::kernel::CitadelWorkspaceService;
use citadel_identity::{base64url_decode, verify_proof, ProofError};
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_types::{
    FailureReason, IdentityOutcome, IdentityRefusal, InternalServiceRequest,
    InternalServiceResponse,
};
use citadel_sdk::prelude::StackedRatchet;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

const ISSUER_ONLY: &[u8] = include_bytes!(
    "../../../../../citadel-identity/tests/contracts/batch-1116/fixtures/well-known-citadel/valid-issuer-only.json"
);
const ORIGIN: &str = "https://work.avarok.net";
const PHISHING: &str = "https://work.avarok.net.evil.example";

fn service(fetch: Arc<dyn WellKnownFetch>) -> (tempfile::TempDir, Svc) {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
        citadel_sdk::prelude::CertVerification::SelfSignedAccepted,
    );
    (
        transfers,
        svc.with_identity(config(SHIPPED_SIGN_LIMITS, fetch, ManualClock::new())),
    )
}

async fn send(svc: &Svc, conn: Uuid, request: serde_json::Value) -> InternalServiceResponse {
    let request: InternalServiceRequest = serde_json::from_value(request).unwrap();
    handle_request(svc, conn, request)
        .await
        .expect("answered")
        .response
}

fn bytes(text: &str) -> serde_json::Value {
    json!(text.as_bytes())
}

async fn create(svc: &Svc) {
    let created = send(
        svc,
        Uuid::new_v4(),
        json!({"IdentityCreate": {"request_id": Uuid::new_v4(), "unlock_password": bytes("pw")}}),
    )
    .await;
    assert!(
        matches!(created, InternalServiceResponse::IdentitySuccess(_)),
        "{created:?}"
    );
}

/// The sign-in page's request, with an `origin` a page might add claiming the issuer's.
fn sign_request() -> serde_json::Value {
    let (nonce, challenge) = ([0x30u8; 32], [0xdau8; 32]);
    json!({"IdentitySignChallenge": {
        "request_id": Uuid::new_v4(), "issuer": "work.avarok.net", "aud": "sw-mcp",
        "nonce": nonce, "challenge": challenge, "unlock_password": bytes("pw"),
        "origin": ORIGIN,
    }})
}

#[tokio::test]
async fn a_page_supplied_origin_is_ignored() {
    let (_dir, svc) = service(Arc::new(NoFetch));
    create(&svc).await;
    let conn = Uuid::new_v4();
    let _entry = svc.control_origins.admit(conn, Some(PHISHING));
    let InternalServiceResponse::IdentitySuccess(success) = send(&svc, conn, sign_request()).await
    else {
        panic!("not signed")
    };
    let IdentityOutcome::SignedChallenge { proof } = success.outcome else {
        panic!("{:?}", success.outcome)
    };
    let blob = base64url_decode(&proof).unwrap();
    let (issuer, aud, nonce, challenge) = ("work.avarok.net", "sw-mcp", [0x30; 32], [0xda; 32]);
    assert!(matches!(
        verify_proof(&blob, issuer, ORIGIN, aud, &nonce, &challenge),
        Err(ProofError::WrongOrigin { .. })
    ));
    let named = verify_proof(&blob, issuer, PHISHING, aud, &nonce, &challenge).unwrap();
    assert_eq!(
        named.origin, PHISHING,
        "the handshake's origin, not the body's"
    );
}

#[tokio::test]
async fn a_connection_without_a_browser_origin_is_refused() {
    let (_dir, svc) = service(Arc::new(NoFetch));
    create(&svc).await;
    let InternalServiceResponse::IdentityFailure(failure) =
        send(&svc, Uuid::new_v4(), sign_request()).await
    else {
        panic!("signed for a native client")
    };
    assert_eq!(failure.reason, IdentityRefusal::NoBrowserOrigin);
}

#[tokio::test]
async fn an_issuer_only_server_is_refused_no_citadel_endpoint() {
    let (_dir, svc) = service(Arc::new(ServeDocument(ISSUER_ONLY)));
    let discovered = send(
        &svc,
        Uuid::new_v4(),
        json!({"ServerDiscovery": {"request_id": Uuid::new_v4(), "server_addr": "work.avarok.net"}}),
    )
    .await;
    let InternalServiceResponse::ServerDiscoveryFailure(failure) = discovered else {
        panic!("{discovered:?}")
    };
    assert_eq!(failure.reason, Some(IdentityRefusal::NoCitadelEndpoint));

    create(&svc).await;
    let request = InternalServiceRequest::RegisterIdentity {
        request_id: Uuid::new_v4(),
        server_addr: "work.avarok.net".into(),
        handle: "alice".into(),
        full_name: "Alice".into(),
        discovery: citadel_internal_service_types::IdentityDiscovery::WellKnown,
        session_security_settings: citadel_sdk::prelude::default_session_security_settings(),
        server_password: None,
        admission_token: None,
    };
    let registered = handle_request(&svc, Uuid::new_v4(), request).await.unwrap();
    let InternalServiceResponse::RegisterFailure(failure) = registered.response else {
        panic!("{:?}", registered.response)
    };
    assert_eq!(failure.reason_code, Some(FailureReason::NoCitadelEndpoint));
}
