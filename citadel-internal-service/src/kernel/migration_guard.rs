//! A page older than the agent never runs beside an account the agent hosts.
//!
//! An account's ILM and conversation store run in exactly one place. Once the
//! agent hosts them (kernel/ilm, kernel/conversations), a page that predates
//! that -- one that never sent `DeclareCapabilities { agent_ilm }` -- would run
//! a second ILM and write the conversation pages itself: the mixed state that
//! corrupts both. So such a page is refused at every door into a hosted
//! session, told to reload, and pushed out of one it was already in when the
//! agent starts hosting it. A page that declared is unaffected; so is any
//! account the agent does not host, which keeps today's behaviour exactly.

use crate::kernel::session_route::announce_roles;
use crate::kernel::store_keys::conversation_owner;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::InternalServiceRequest;
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::Ratchet;
use uuid::Uuid;

/// What an older page is told. It names the remedy, because the user can act on it.
pub(crate) const OLDER_PAGE: &str =
    "This page is older than your Citadel agent. Reload it to continue.";

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// May `connection` join `cid`? Only if the agent does not host it, or the
    /// page declared that it uses the agent's ILM.
    pub(crate) fn refuses_older_page(&self, cid: u64, connection: Uuid) -> bool {
        let refused = self.ilm_hosts.is_hosted(cid) && !self.wants_agent_ilm(connection);
        if refused {
            warn!(target: "citadel", "[MIGRATION] connection {connection} predates agent hosting; refused session {cid}");
        }
        refused
    }

    /// The agent has just started hosting `cid`: any member that has not
    /// declared leaves, and is told it is detached.
    pub(crate) fn displace_older_pages(&self, cid: u64) {
        let Some(subscribers) = crate::kernel::membership::subscribers_of(self, cid) else {
            return;
        };
        let older: Vec<Uuid> = subscribers
            .members()
            .into_iter()
            .filter(|member| !self.wants_agent_ilm(*member))
            .collect();
        for member in &older {
            subscribers.release(*member);
        }
        if !older.is_empty() {
            warn!(target: "citadel", "[MIGRATION] session {cid} is now agent-hosted; detached older pages {older:?}");
            announce_roles(&self.tx_to_localhost_clients, cid, &subscribers, &older);
        }
    }

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
