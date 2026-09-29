//! A PostConnect reaching this handler is either a peer's offer or an answer to
//! one of ours. Only the offer is a connect request.
//!
//! When the peer declines, the SDK reports it twice: a `SignalError` the dial's
//! ticket listener consumes, then the PostConnect itself carrying
//! `invitee_response: Some(Decline)`. The second is forwarded under the same
//! ticket, and whether it reaches the listener or falls through to here depends
//! on whether the dial has already dropped the listener. Read as an offer, it
//! handed the decliner's own refusal back to the dialler as an incoming request
//! (CI: `peer_security_level::a_declined_low_offer_is_replaced_by_the_higher_one`).

use super::handle;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_sdk::prelude::{
    PeerConnectionType, PeerEvent, PeerResponse, PeerSignal, SessionSecuritySettings,
    StackedRatchet, Ticket, UdpMode,
};

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

const DIALLER: u64 = 1;
const DECLINER: u64 = 2;

/// The PostConnect as the SDK hands it to the dialler: the conn is the
/// decliner's (it crafted the answer), so from the dialler's side it looks
/// exactly like an offer from the decliner. Only `invitee_response` differs.
fn post_connect_to_dialler(invitee_response: Option<PeerResponse>) -> PeerEvent {
    PeerEvent {
        event: PeerSignal::PostConnect {
            peer_conn_type: PeerConnectionType::LocalGroupPeer {
                session_cid: DECLINER,
                peer_cid: DIALLER,
            },
            ticket_opt: Some(Ticket(7)),
            invitee_response,
            session_security_settings: SessionSecuritySettings::default(),
            udp_mode: UdpMode::Disabled,
            session_password: None,
        },
        ticket: Ticket(7),
        session_cid: DIALLER,
    }
}

#[tokio::test]
async fn a_declined_answer_is_not_stored_as_an_incoming_offer() {
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory();
    handle(&svc, post_connect_to_dialler(Some(PeerResponse::Decline)))
        .await
        .unwrap();
    assert!(
        svc.pending_peer_connect_signals.read().is_empty(),
        "the peer's refusal was stored as a pending offer from it"
    );
}

/// The control: the same signal without a response IS an offer, so the test
/// above is measuring the response, not a handler that stores nothing.
#[tokio::test]
async fn an_offer_is_stored_for_acceptance() {
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory();
    handle(&svc, post_connect_to_dialler(None)).await.unwrap();
    assert!(svc
        .pending_peer_connect_signals
        .read()
        .contains_key(&(DIALLER, DECLINER)));
}
