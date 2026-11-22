use super::*;
use citadel_sdk::prelude::{
    GroupDelivery, GroupMessageRefusal, GroupStateSource, NotDelivered, NotRelayed,
};

const KEY: MessageGroupKey = MessageGroupKey { cid: 7, mgid: 3 };

fn answered(broadcast: GroupBroadcast) -> InternalServiceResponse {
    answer(42, &broadcast, None).expect("the broadcast is one the window is told of")
}

#[test]
fn a_delivery_report_reaches_the_window_member_by_member() {
    let delivery = GroupDelivery::Relayed {
        delivered: vec![8],
        stored: vec![9],
        not_delivered: vec![(10, NotDelivered::BacklogFull), (11, NotDelivered::Offline)],
    };
    let InternalServiceResponse::GroupMessageResponse(report) =
        answered(GroupBroadcast::MessageResponse {
            key: KEY,
            delivery: delivery.clone(),
        })
    else {
        panic!("expected a delivery report");
    };
    assert_eq!(
        (report.cid, report.group_key, report.request_id),
        (42, KEY, None)
    );
    assert_eq!(report.delivery, delivery);
}

#[test]
fn a_message_that_reached_no_one_says_why() {
    let InternalServiceResponse::GroupMessageResponse(report) =
        answered(GroupBroadcast::MessageResponse {
            key: KEY,
            delivery: GroupDelivery::NotRelayed(NotRelayed::NoGroupKey),
        })
    else {
        panic!("expected a delivery report");
    };
    assert_eq!(
        report.delivery,
        GroupDelivery::NotRelayed(NotRelayed::NoGroupKey)
    );
}

#[test]
fn every_refusal_names_the_sender_the_group_and_the_reason() {
    for refusal in [
        GroupMessageRefusal::TooLarge {
            bytes: 20_000,
            max_bytes: 16_384,
        },
        GroupMessageRefusal::RateLimited { max_per_second: 4 },
        GroupMessageRefusal::NotAMember,
    ] {
        let InternalServiceResponse::GroupMessageRefusedNotification(n) =
            answered(GroupBroadcast::MessageRefused { key: KEY, refusal })
        else {
            panic!("expected a refusal");
        };
        assert_eq!(
            (n.cid, n.group_key, n.refusal, n.request_id),
            (42, KEY, refusal, None)
        );
    }
}

#[test]
fn a_transfer_names_the_new_owner_and_the_one_that_left() {
    let InternalServiceResponse::GroupOwnershipTransferredNotification(n) =
        answered(GroupBroadcast::OwnershipTransferred {
            key: KEY,
            owner: 11,
            previous: 7,
            tree: 2,
        })
    else {
        panic!("expected an ownership transfer");
    };
    assert_eq!((n.cid, n.group_key, n.owner, n.previous), (42, KEY, 11, 7));
}

#[test]
fn a_refused_kick_or_add_is_a_failure_and_an_accepted_one_is_not_reported() {
    let kicked = |success| GroupBroadcast::KickResponse { key: KEY, success };
    assert!(matches!(
        answered(kicked(false)),
        InternalServiceResponse::GroupKickFailure(GroupKickFailure { cid: 42, .. })
    ));
    assert!(answer(42, &kicked(true), None).is_none());

    let added = |failed_to_invite_list| GroupBroadcast::AddResponse {
        key: KEY,
        failed_to_invite_list,
    };
    let InternalServiceResponse::GroupInviteFailure(failure) = answered(added(Some(vec![9, 10])))
    else {
        panic!("expected an invite failure");
    };
    assert!(
        failure.message.ends_with("not invited: 9, 10"),
        "{}",
        failure.message
    );
    assert!(answer(42, &added(Some(vec![])), None).is_none());
    assert!(answer(42, &added(None), None).is_none());
}

#[test]
fn a_taken_leave_is_reported_and_a_refused_one_is_not() {
    let left = |success| GroupBroadcast::LeaveRoomResponse {
        key: KEY,
        success,
        message: "left".to_string(),
    };
    let InternalServiceResponse::GroupLeaveNotification(n) = answered(left(true)) else {
        panic!("expected a leave notification");
    };
    assert_eq!(
        (n.cid, n.group_key, n.success, n.request_id),
        (42, KEY, true, None)
    );
    assert!(answer(42, &left(false), None).is_none());
}

#[test]
fn a_report_carries_the_send_it_answers() {
    let send = uuid::Uuid::new_v4();
    let report = GroupBroadcast::MessageResponse {
        key: KEY,
        delivery: GroupDelivery::NotRelayed(NotRelayed::SessionEnded),
    };
    let Some(InternalServiceResponse::GroupMessageResponse(report)) =
        answer(42, &report, Some(send))
    else {
        panic!("expected a delivery report");
    };
    assert_eq!(report.request_id, Some(send));
}

#[test]
fn a_restore_says_where_the_state_came_from() {
    for source in [
        GroupStateSource::ThisDevice,
        GroupStateSource::Backup,
        GroupStateSource::Rejoined,
        GroupStateSource::Refounded,
    ] {
        let InternalServiceResponse::GroupRestoredNotification(n) =
            answered(GroupBroadcast::Restored { key: KEY, source })
        else {
            panic!("expected a restore notice");
        };
        assert_eq!(
            (n.cid, n.group_key, n.source, n.request_id),
            (42, KEY, source, None)
        );
    }
}
