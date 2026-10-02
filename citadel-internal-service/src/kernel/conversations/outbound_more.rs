//! Reacting and marking read, on a window's behalf.

use super::command::{self, AckKind};
use super::engine::{preferences, store, Change, Engine};
use super::envelope::raw_envelope;
use super::io::ConversationIo;
use super::outbound::Outcome;
use citadel_internal_service_types::{ConversationEventKind, Reaction};

impl Engine {
    pub(crate) async fn react(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        id: &str,
        emoji: String,
        active: bool,
    ) -> Outcome {
        let _held = self.lock(cid, peer).await;
        let at = io.now_ms();
        let change = Reaction {
            emoji: emoji.clone(),
            reactor_cid: cid,
            at,
            active,
        };
        let Some(message) = store(io, cid).react(peer, id, &change).await? else {
            return Ok(None);
        };
        self.announce(
            io,
            cid,
            peer,
            Change::message(ConversationEventKind::Updated, message.clone()),
        )
        .await;
        let envelope = raw_envelope(io, cid, peer).await;
        io.send_p2p(
            cid,
            peer,
            command::messaging_layer(command::reaction_layer(id, &emoji, active, at), &envelope),
        )
        .await
        .map_err(|e| format!("Saved here, but not sent: {e}"))?;
        Ok(Some(message))
    }

    /// Everything from `peer` is read; receipts go out if the account sends them.
    pub(crate) async fn mark_read(&self, io: &dyn ConversationIo, cid: u64, peer: u64) -> Outcome {
        let _held = self.lock(cid, peer).await;
        let (read, metadata) = store(io, cid).mark_read(peer, io.now_ms()).await?;
        let send_receipts = preferences(io, cid).await?.send_read_receipts;
        for message in &read {
            self.announce(
                io,
                cid,
                peer,
                Change::message(ConversationEventKind::Updated, message.clone()),
            )
            .await;
            if send_receipts {
                let receipt = command::ack(AckKind::Read, &message.id, io.now_ms());
                if let Err(e) = io.send_p2p(cid, peer, receipt).await {
                    citadel_sdk::logging::warn!(target: "citadel", "[CONVERSATIONS] {cid}: read receipt for {} not sent: {e}", message.id);
                }
            }
        }
        if let Some(metadata) = metadata {
            let change = Change {
                kind: ConversationEventKind::MetadataChanged,
                message: None,
                message_id: None,
                metadata: Some(metadata),
                request_id: None,
            };
            self.announce(io, cid, peer, change).await;
        }
        Ok(None)
    }
}
