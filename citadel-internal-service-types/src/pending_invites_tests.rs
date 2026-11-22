//! `GroupListJoinedSuccess.pending_invites` and `owners` on the wire.
use super::*;

#[test]
fn a_list_without_its_invites_is_refused() {
    let missing = r#"{"cid":7,"groups":[],"request_id":null,"owners":[]}"#;
    let err = serde_json::from_str::<GroupListJoinedSuccess>(missing).unwrap_err();
    assert!(err.to_string().contains("pending_invites"), "{err}");
}

#[test]
fn a_pending_invite_round_trips() {
    let answer = GroupListJoinedSuccess {
        cid: 7,
        groups: vec![],
        request_id: None,
        pending_invites: vec![PendingGroupInvite {
            peer_cid: 42,
            group_key: MessageGroupKey::new(42, 9),
        }],
        owners: vec![JoinedGroupOwner {
            group_key: MessageGroupKey::new(42, 9),
            owner: 43,
        }],
    };
    let back: GroupListJoinedSuccess =
        serde_json::from_str(&serde_json::to_string(&answer).unwrap()).unwrap();
    assert_eq!(back.pending_invites, answer.pending_invites);
    assert_eq!(back.owners, answer.owners);
}

#[test]
fn a_list_without_its_owners_is_refused() {
    let missing = r#"{"cid":7,"groups":[],"request_id":null,"pending_invites":[]}"#;
    let err = serde_json::from_str::<GroupListJoinedSuccess>(missing).unwrap_err();
    assert!(err.to_string().contains("owners"), "{err}");
}
