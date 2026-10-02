//! A peer's screenshot notice, stored as a system notice if the account asks.
//! Ported from inbound-screenshot-notice.ts.

use super::command::Envelope;
use super::engine::{preferences, store, Change, Engine};
use super::envelope::received;
use super::io::ConversationIo;
use citadel_internal_service_types::{
    ConversationEventKind, ConversationMessage, MessageStatus, MessageType,
};

impl Engine {
    pub(super) async fn screenshot(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        envelope: Envelope,
        taken_at: Option<f64>,
    ) -> bool {
        let Ok(prefs) = preferences(io, cid).await else {
            return false;
        };
        if !prefs.notify_on_screenshot {
            return true;
        }
        let name = store(io, cid)
            .load_metadata(peer)
            .await
            .ok()
            .flatten()
            .and_then(|m| m.peer_username)
            .unwrap_or_else(|| peer.to_string());
        let message = ConversationMessage {
            sender_cid: peer,
            recipient_cid: cid,
            status: MessageStatus::Delivered,
            message_type: MessageType::SystemNotice,
            ..received(
                &envelope,
                format!("{name} may have taken a screenshot"),
                taken_at.unwrap_or_else(|| io.now_ms()),
            )
        };
        let appended = {
            let _held = self.lock(cid, peer).await;
            store(io, cid)
                .append(peer, &message, None, io.now_ms())
                .await
        };
        match appended {
            Ok(Some(metadata)) => {
                let mut change = Change::message(ConversationEventKind::Appended, message);
                change.metadata = Some(metadata);
                self.announce(io, cid, peer, change).await;
                true
            }
            Ok(None) => true,
            Err(_) => false,
        }
    }
}
