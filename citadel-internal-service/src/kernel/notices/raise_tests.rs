//! Which session notifications deserve a notice, read from the notification.

use super::source_of;
use crate::kernel::notices::decide::NoticeSource;
use citadel_internal_service_types::*;

const ME: u64 = 7;
const BOB: u64 = 9;

fn offer() -> InternalServiceResponse {
    InternalServiceResponse::FileTransferRequestNotification(FileTransferRequestNotification {
        cid: ME,
        peer_cid: BOB,
        metadata: VirtualObjectMetadata {
            name: "plans.pdf".into(),
            date_created: String::new(),
            author: String::new(),
            plaintext_length: 1,
            group_count: 1,
            object_id: ObjectId(1),
            cid: BOB,
            transfer_type: TransferType::FileTransfer,
        },
        request_id: None,
    })
}

#[test]
fn a_peer_request_an_invite_and_a_file_offer_are_noticed() {
    let request = InternalServiceResponse::PeerRegisterNotification(PeerRegisterNotification {
        cid: ME,
        peer_cid: BOB,
        peer_username: "bob".into(),
        request_id: None,
    });
    assert_eq!(
        source_of(&request),
        Some((
            ME,
            NoticeSource::PeerRequest {
                peer: BOB,
                peer_username: Some("bob".into())
            }
        ))
    );
    let invite = InternalServiceResponse::GroupInviteNotification(GroupInviteNotification {
        cid: ME,
        peer_cid: BOB,
        group_key: MessageGroupKey { cid: BOB, mgid: 1 },
        request_id: None,
    });
    assert_eq!(
        source_of(&invite),
        Some((
            ME,
            NoticeSource::GroupInvite {
                peer: BOB,
                peer_username: None
            }
        ))
    );
    assert_eq!(
        source_of(&offer()),
        Some((
            ME,
            NoticeSource::FileOffer {
                peer: BOB,
                peer_username: None,
                file_name: "plans.pdf".into()
            }
        ))
    );
}

#[test]
fn anything_else_is_not() {
    let ended = InternalServiceResponse::DisconnectNotification(DisconnectNotification {
        cid: ME,
        peer_cid: Some(BOB),
        request_id: None,
    });
    assert_eq!(source_of(&ended), None);
}
