//! `GroupListJoinedSuccess.pending_invites` across agent versions.
use super::*;

#[test]
fn an_older_agents_list_still_parses_and_says_nothing_about_invites() {
    let old = r#"{"cid":7,"groups":[],"request_id":null}"#;
    let parsed: GroupListJoinedSuccess =
        serde_json::from_str(old).expect("an old answer must parse");
    assert_eq!(parsed.pending_invites, None);
}

#[test]
fn a_pending_invite_round_trips() {
    let answer = GroupListJoinedSuccess {
        cid: 7,
        groups: vec![],
        request_id: None,
        pending_invites: Some(vec![PendingGroupInvite {
            peer_cid: 42,
            group_key: MessageGroupKey::new(42, 9),
        }]),
    };
    let back: GroupListJoinedSuccess =
        serde_json::from_str(&serde_json::to_string(&answer).unwrap()).unwrap();
    assert_eq!(back.pending_invites, answer.pending_invites);
}
