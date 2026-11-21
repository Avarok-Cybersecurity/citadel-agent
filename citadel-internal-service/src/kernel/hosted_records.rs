//! The agent is the only writer of a hosted account's conversation records
//! (kernel/conversations): a window's LocalDB write to one is refused.

use crate::kernel::store_keys::conversation_owner;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::prelude::Ratchet;

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// A write to a hosted account's conversation records from outside the
    /// agent: refused, because the agent is their only writer.
    pub(crate) fn writes_hosted_conversation(&self, command: &InternalServiceRequest) -> bool {
        let key = match command {
            InternalServiceRequest::LocalDBSetKV { cid: 0, key, .. }
            | InternalServiceRequest::LocalDBDeleteKV { cid: 0, key, .. } => key,
            _ => return false,
        };
        conversation_owner(key).is_some_and(|owner| self.ilm_hosts.is_hosted(owner))
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::store_keys::conversation_owner;

    #[test]
    fn a_conversation_key_names_its_owning_account() {
        assert_eq!(
            conversation_owner("msgs_with_peer_1001_with_2002_metadata"),
            Some(1001)
        );
        assert_eq!(
            conversation_owner("msgs_with_peer_1001_with_2002_3"),
            Some(1001)
        );
        // A record from before keys were scoped names only the peer: not attributable.
        assert_eq!(conversation_owner("msgs_with_peer_2002_metadata"), None);
        assert_eq!(conversation_owner("citadel_sessions"), None);
        assert_eq!(conversation_owner("msgs_with_peer_x_with_2"), None);
    }
}
