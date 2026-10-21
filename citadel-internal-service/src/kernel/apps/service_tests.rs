//! The apps requests through the agent's real dispatcher: who is refused, and what an
//! arriving envelope becomes. An in-memory service, no SDK session, no network.

use crate::kernel::apps::ihttp::envelope;
use crate::kernel::conversations::cbor::Value;
use crate::kernel::requests::handle_request;
use crate::kernel::CitadelWorkspaceService;
use citadel_ihttp_types::{encode, IhttpFrame, IhttpFrameKind};
use citadel_internal_service_connector::io_interface::in_memory::InMemoryInterface;
use citadel_internal_service_types::*;
use citadel_sdk::prelude::StackedRatchet;
use uuid::Uuid;

type Svc = CitadelWorkspaceService<InMemoryInterface, StackedRatchet>;

fn service() -> (tempfile::TempDir, Svc) {
    let transfers = tempfile::tempdir().expect("temp dir");
    let (_connector, svc): (_, Svc) = CitadelWorkspaceService::new_in_memory(
        crate::SERVER_RECONNECT,
        crate::BrowserTransferRoot::at(transfers.path().to_path_buf()),
    );
    (transfers, svc)
}

fn fetch(cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::AppFetch {
        request_id: Uuid::new_v4(),
        cid,
        app_id: "com.acme.kanban".into(),
        install_id: "inst-1".into(),
        grant: AppNetGrant {
            hosts: vec!["api.acme.com".into()],
            private: false,
        },
        request: AppHttpRequest {
            method: "GET".into(),
            url: "https://api.acme.com/".into(),
            headers: vec![],
            body: vec![],
        },
        stream: false,
    }
}

fn end_frame() -> Vec<u8> {
    encode(&IhttpFrame {
        v: 1,
        stream: 1,
        kind: IhttpFrameKind::End,
    })
    .expect("valid")
}

async fn answer(svc: &Svc, request: InternalServiceRequest) -> Option<InternalServiceResponse> {
    handle_request(svc, Uuid::new_v4(), request)
        .await
        .map(|handled| handled.response)
}

/// cid 0 passes the ownership gate (it names the agent's scratch store, not a session), and
/// an unknown cid is refused by it: both must end in `Session`, the first by the handlers'
/// own check.
#[tokio::test]
async fn no_apps_request_acts_without_an_attached_session() {
    let (_dir, svc) = service();
    for cid in [0, 99] {
        let session = AppFetchRefusal::Unavailable(AppFetchUnavailable::Session);
        match answer(&svc, fetch(cid)).await {
            Some(InternalServiceResponse::AppFetchFailure(f)) => assert_eq!(f.refusal, session),
            other => panic!("cid {cid}: {other:?}"),
        }
        let kv = InternalServiceRequest::AppKv {
            request_id: Uuid::new_v4(),
            cid,
            namespace: AppKvNamespace {
                app_id: "com.acme.kanban".into(),
                scope_key: "workspace".into(),
            },
            op: AppKvOp::Get { key: "k".into() },
        };
        match answer(&svc, kv).await {
            Some(InternalServiceResponse::AppKvFailure(f)) => {
                assert_eq!(f.refusal, AppKvRefusal::Session)
            }
            other => panic!("cid {cid}: {other:?}"),
        }
        let send = InternalServiceRequest::IhttpSend {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid: 5,
            frame: end_frame(),
        };
        match answer(&svc, send).await {
            Some(InternalServiceResponse::IhttpSendFailure(f)) => {
                assert_eq!(f.refusal, IhttpSendRefusal::Session)
            }
            other => panic!("cid {cid}: {other:?}"),
        }
        let credit = InternalServiceRequest::AppFetchCredit {
            request_id: Uuid::new_v4(),
            cid,
            fetch_id: Uuid::new_v4(),
            n: 4,
        };
        assert!(answer(&svc, credit).await.is_none());
    }
}

#[tokio::test]
async fn an_envelope_sent_as_a_plain_message_is_refused() {
    let (_dir, svc) = service();
    let wrapped = Value::object(vec![
        ("type", Value::text(citadel_ihttp_types::ENVELOPE_KIND)),
        ("payload", Value::Bytes(end_frame())),
    ])
    .encode();
    let send = |message: Vec<u8>| InternalServiceRequest::SendReliable {
        request_id: Uuid::new_v4(),
        cid: 0,
        peer_cid: 5,
        message,
        security_level: SecurityLevel::Standard,
        compression_hint: None,
    };
    match answer(&svc, send(wrapped)).await {
        Some(InternalServiceResponse::MessageSendFailure(f)) => {
            assert!(f.message.contains("IhttpSend"), "{}", f.message)
        }
        other => panic!("{other:?}"),
    }
    // The control: ordinary bytes reach the handler, which refuses for its own reason.
    match answer(&svc, send(b"hello".to_vec())).await {
        Some(InternalServiceResponse::MessageSendFailure(f)) => {
            assert!(!f.message.contains("IhttpSend"), "{}", f.message)
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_arriving_envelope_never_reaches_the_conversation_store() {
    let (_dir, svc) = service();
    let notification = |message: Vec<u8>| MessageNotification {
        message,
        cid: 7,
        peer_cid: 5,
        request_id: None,
    };
    let good = envelope::wrap(&IhttpFrame {
        v: 1,
        stream: 1,
        kind: IhttpFrameKind::End,
    })
    .expect("wraps");
    assert!(svc.deliver_ihttp(notification(good)).is_ok());
    let mut bad = envelope::wrap(&IhttpFrame {
        v: 1,
        stream: 1,
        kind: IhttpFrameKind::End,
    })
    .expect("wraps");
    bad.push(0);
    assert!(svc.deliver_ihttp(notification(bad)).is_ok());
    let counts = svc.apps.ihttp.read();
    assert_eq!(
        (counts.no_receiver, counts.malformed_in, counts.received),
        (1, 1, 0)
    );
    // Anything else is given back, untouched, for the conversation store.
    let chat = b"not an envelope".to_vec();
    let returned = svc.deliver_ihttp(notification(chat.clone()));
    assert_eq!(returned.map_err(|n| n.message), Err(chat));
}

/// Through the hosted ILM's delivery seam: an envelope is consumed by the relay (no window
/// takes it here, so it is counted as such), never offered to the conversation store, which
/// would hand it to windows as a message.
#[tokio::test]
async fn the_hosted_ilm_hands_an_envelope_to_the_relay() {
    use crate::kernel::ilm::HostIo;
    let (_dir, svc) = service();
    let wrapped = envelope::wrap(&IhttpFrame {
        v: 1,
        stream: 3,
        kind: IhttpFrameKind::End,
    })
    .expect("wraps");
    let notification = MessageNotification {
        message: wrapped,
        cid: 7,
        peer_cid: 5,
        request_id: None,
    };
    assert!(HostIo::deliver(&svc, 7, notification).await, "not consumed");
    assert_eq!(svc.apps.ihttp.read().no_receiver, 1);
}
