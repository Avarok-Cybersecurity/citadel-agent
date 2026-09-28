//! Group invitations this session has not answered.
//!
//! An invitation reaches the agent as a `GroupBroadcast::Invitation` and was only ever forwarded
//! to whichever browser connection owned the session at that moment. With no tab open it went
//! nowhere, and nothing kept it: an invite sent while the invitee was away was never shown to
//! them (found live, two Macs, 2026-09-28). The agent is the one party that is always there, so
//! it keeps the invitation with the session until it is answered, the group ends, or the session
//! is removed, and `GroupListJoined` -- which the UI sends once its group handlers are bound --
//! hands the unanswered ones back.
use citadel_internal_service_types::PendingGroupInvite;
use citadel_sdk::prelude::MessageGroupKey;

#[derive(Default)]
pub(crate) struct PendingGroupInvites {
    /// In arrival order, so the UI lists them as they came. One entry per group: a second
    /// invitation to the same group replaces the first (the inviter is whoever asked last).
    invites: Vec<(MessageGroupKey, u64)>,
}

impl PendingGroupInvites {
    pub(crate) fn record(&mut self, key: MessageGroupKey, inviter: u64) {
        match self.invites.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = inviter,
            None => self.invites.push((key, inviter)),
        }
    }

    /// The invitation is answered, or its group is gone. True when one was pending.
    pub(crate) fn settle(&mut self, key: &MessageGroupKey) -> bool {
        let before = self.invites.len();
        self.invites.retain(|(k, _)| k != key);
        before != self.invites.len()
    }

    /// Unanswered invitations, leaving out any group this session is already in.
    pub(crate) fn unanswered(&self, joined: &[MessageGroupKey]) -> Vec<PendingGroupInvite> {
        self.invites
            .iter()
            .filter(|(key, _)| !joined.contains(key))
            .map(|(group_key, peer_cid)| PendingGroupInvite {
                peer_cid: *peer_cid,
                group_key: *group_key,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u128) -> MessageGroupKey {
        MessageGroupKey::new(7, n)
    }

    #[test]
    fn an_invitation_is_kept_until_it_is_answered() {
        let mut pending = PendingGroupInvites::default();
        pending.record(key(1), 42);
        pending.record(key(2), 43);
        assert_eq!(
            pending.unanswered(&[]),
            vec![
                PendingGroupInvite {
                    peer_cid: 42,
                    group_key: key(1)
                },
                PendingGroupInvite {
                    peer_cid: 43,
                    group_key: key(2)
                },
            ]
        );
        assert!(pending.settle(&key(1)));
        assert!(!pending.settle(&key(1)), "settled twice");
        assert_eq!(pending.unanswered(&[]).len(), 1);
    }

    #[test]
    fn a_second_invitation_to_one_group_is_one_entry() {
        let mut pending = PendingGroupInvites::default();
        pending.record(key(1), 42);
        pending.record(key(1), 44);
        assert_eq!(
            pending.unanswered(&[]),
            vec![PendingGroupInvite {
                peer_cid: 44,
                group_key: key(1)
            }]
        );
    }

    #[test]
    fn a_group_already_joined_is_not_offered_again() {
        let mut pending = PendingGroupInvites::default();
        pending.record(key(1), 42);
        assert!(pending.unanswered(&[key(1)]).is_empty());
    }
}
