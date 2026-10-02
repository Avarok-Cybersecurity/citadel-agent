//! The agent answers a peer's PeerConnect itself for an account it hosts.
//!
//! An offer used to complete only when a window showing that exact account sent
//! `PeerConnectAccept`. An account signed in with no such window -- the landing
//! page, another account on screen, every window closed -- left every offer
//! unanswered, and the initiator timed out waiting for the channel, for as long
//! as it kept trying. A hosted account is one the agent already serves with no
//! window (its ILM and conversations, multi-window mw3/mw4), so it answers
//! connects too, by the UI's own rules (`decide`), from records it holds: the
//! pause record is the UI's own LocalDB key, and the chat's minimum level comes
//! with the account preferences windows push.
//!
//! The notification tells windows the agent has the offer (`answered_by_agent`),
//! so exactly one answer is sent. An account the agent does not host is left to
//! windows, as before.

mod decide;
#[cfg(test)]
mod tests;

use crate::kernel::conversations::engine::preferences;
use crate::kernel::requests::answer_local_db;
use crate::kernel::requests::peer::answer::{answer_offer, window_relay};
use crate::kernel::store_keys::pause_key;
use crate::kernel::CitadelWorkspaceService;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{
    InternalServiceRequest, InternalServiceResponse, SecurityLevel, KEY_NOT_FOUND,
};
use citadel_sdk::logging::{info, warn};
use citadel_sdk::prelude::Ratchet;
pub(crate) use decide::AgentAnswer;
use decide::{decide, OfferFacts, PauseRecord};
use uuid::Uuid;

impl<T: IOInterface + Sync, R: Ratchet> CitadelWorkspaceService<T, R> {
    /// How `peer_cid`'s offer to session `cid`, at `offered`, is answered.
    pub(crate) async fn agent_answer_for(
        &self,
        cid: u64,
        peer_cid: u64,
        offered: SecurityLevel,
    ) -> AgentAnswer {
        if !self.ilm_hosts.is_hosted(cid) {
            return AgentAnswer::LeaveToWindows;
        }
        let registered = match self
            .remote()
            .account_manager()
            .get_hyperlan_peer_list(cid)
            .await
        {
            Ok(peers) => Some(peers.is_some_and(|peers| peers.contains(&peer_cid))),
            Err(err) => {
                warn!(target: "citadel", "[INBOUND-CONNECT] {cid}: peer list unreadable ({err:?})");
                None
            }
        };
        let pause = PauseRecord::from_read(&self.session_kv_get(cid, pause_key(peer_cid)).await);
        let minimum = preferences(self, cid)
            .await
            .inspect_err(|err| warn!(target: "citadel", "[INBOUND-CONNECT] {cid}: preferences unreadable ({err})"))
            .ok()
            .map(|prefs| prefs.security_minimum_for(peer_cid).sdk());
        let answer = decide(OfferFacts {
            registered,
            pause,
            offered,
            minimum,
        });
        info!(target: "citadel", "[INBOUND-CONNECT] {cid} <- {peer_cid}: {answer:?} ({pause:?}, registered {registered:?})");
        answer
    }

    /// Send the agent's answer, if it is one that sends anything.
    pub(crate) async fn answer_as_agent(&self, cid: u64, peer_cid: u64, answer: AgentAnswer) {
        let accept = match answer {
            AgentAnswer::Accept => true,
            AgentAnswer::Decline => false,
            AgentAnswer::NoAnswer | AgentAnswer::LeaveToWindows => return,
        };
        let relay = window_relay(self, cid);
        if let Err(err) = answer_offer(self, cid, peer_cid, accept, relay.as_ref(), None).await {
            warn!(target: "citadel", "[INBOUND-CONNECT] {cid} <- {peer_cid}: answer not sent: {err}");
        }
    }

    /// One key of the session's own LocalDB, as a window reads it.
    async fn session_kv_get(&self, cid: u64, key: String) -> Result<Option<Vec<u8>>, String> {
        let request = InternalServiceRequest::LocalDBGetKV {
            request_id: Uuid::new_v4(),
            cid,
            peer_cid: None,
            key,
        };
        match answer_local_db(self, request).await {
            InternalServiceResponse::LocalDBGetKVSuccess(ok) => Ok(Some(ok.value)),
            InternalServiceResponse::LocalDBGetKVFailure(f) if f.message == KEY_NOT_FOUND => {
                Ok(None)
            }
            InternalServiceResponse::LocalDBGetKVFailure(f) => Err(f.message),
            other => Err(format!("unexpected answer: {other:?}")),
        }
    }
}
