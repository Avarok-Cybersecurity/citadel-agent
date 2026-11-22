use super::*;

const KEY: MessageGroupKey = MessageGroupKey { cid: 7, mgid: 3 };

#[test]
fn a_message_to_a_group_without_a_channel_reaches_the_window() {
    let groups = GroupChannels::new();
    let Some(InternalServiceResponse::GroupMessageNotification(heard)) =
        message(&groups, 42, KEY, 8, b"held".to_vec())
    else {
        panic!("a member whose channel was dropped is still in the group");
    };
    assert_eq!((heard.cid, heard.peer_cid, heard.group_key), (42, 8, KEY));
    assert_eq!(heard.message, b"held");
}

#[test]
fn the_end_of_a_group_without_a_channel_reaches_the_window() {
    let mut groups = GroupChannels::new();
    let Some(InternalServiceResponse::GroupDisconnectNotification(ended)) =
        disconnected(&mut groups, 42, KEY, GroupEndReason::Removed)
    else {
        panic!("the end of a group this session holds no channel for must still be told");
    };
    assert_eq!((ended.cid, ended.group_key), (42, KEY));
    assert_eq!(ended.reason, GroupEndReason::Removed);
}
