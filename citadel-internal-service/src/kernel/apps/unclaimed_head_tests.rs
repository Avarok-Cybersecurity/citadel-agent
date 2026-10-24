//! An arriving internal-HTTP head that no window takes, through the agent's real delivery.

use super::service_tests::service;
use crate::kernel::apps::ihttp::envelope;
use citadel_ihttp_types::{IhttpFrame, IhttpFrameKind};
use citadel_internal_service_types::MessageNotification;

/// A head no window takes is refused back to its caller as a window refusing it would be,
/// so the caller does not wait out its deadline; any other frame is only dropped.
#[tokio::test]
async fn a_head_no_window_takes_is_answered_with_a_refusal() {
    use crate::kernel::apps_io::IhttpDelivered;
    use citadel_ihttp_types::{decode, CancelReason, IhttpMethod};
    let (_dir, svc) = service();
    let head = envelope::wrap(&IhttpFrame {
        v: 1,
        stream: 9,
        kind: IhttpFrameKind::RequestHead {
            method: IhttpMethod::Get,
            path: "/items".into(),
            query: None,
            headers: vec![],
            body_len: Some(0),
            app_id: "com.acme.kanban".into(),
            scope_key: "node:1".into(),
        },
    })
    .expect("wraps");
    let arrived = MessageNotification {
        message: head,
        cid: 7,
        peer_cid: 5,
        request_id: None,
    };
    let Ok(IhttpDelivered::Refused { peer_cid, answer }) = svc.deliver_ihttp(arrived) else {
        panic!("a head nobody took was not answered")
    };
    assert_eq!(peer_cid, 5);
    let envelope::Inbound::Frame(frame) = envelope::read(&answer) else {
        panic!("the answer is not an envelope")
    };
    assert_eq!(
        decode(&frame).expect("a frame"),
        IhttpFrame {
            v: 1,
            stream: 9,
            kind: IhttpFrameKind::Cancel {
                reason: CancelReason::Refused
            },
        }
    );
    // The control: a frame of a stream never opened here gets no answer.
    let end = MessageNotification {
        message: envelope::wrap(&IhttpFrame {
            v: 1,
            stream: 9,
            kind: IhttpFrameKind::End,
        })
        .expect("wraps"),
        cid: 7,
        peer_cid: 5,
        request_id: None,
    };
    assert!(matches!(
        svc.deliver_ihttp(end),
        Ok(IhttpDelivered::Consumed)
    ));
}
