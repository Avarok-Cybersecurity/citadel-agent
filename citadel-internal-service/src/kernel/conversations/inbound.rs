//! A P2P message the account's ILM delivered: stored, acknowledged, announced.
//!
//! Ported from the web UI's message-handler-routing.ts and the inbound handlers
//! it dispatches to, which no longer write: the agent does, once, whether or
//! not a window is open. What it returns is ILM's acknowledgement: `true`
//! means "handled, retire it", `false` means "not yet, deliver it again".

use super::command::{self, AckKind, Envelope, Inbound};
use super::engine::{preferences, store, Change, Engine};
use super::envelope::{raw_envelope, received};
use super::io::ConversationIo;
use super::store_mutations::{Revised, Revision};
use citadel_internal_service_types::{
    ConversationEventKind, ConversationMessage, InternalServiceResponse, MessageNotification,
    MessageStatus, Reaction,
};
use citadel_sdk::logging::warn;

impl Engine {
    pub(crate) async fn delivered(&self, io: &dyn ConversationIo, n: MessageNotification) -> bool {
        let (cid, peer) = (n.cid, n.peer_cid);
        match command::read(&n.message) {
            // Not the store's (file transfer, RE-VFS, anything unknown): only a
            // window can act on it, so it waits for one.
            None | Some(Inbound::ForAWindow) => {
                io.publish(cid, InternalServiceResponse::MessageNotification(n)) > 0
            }
            // Of use now or never.
            Some(Inbound::Ephemeral) => {
                io.publish(cid, InternalServiceResponse::MessageNotification(n));
                true
            }
            // The account is here whenever its agent is: answer for it.
            Some(Inbound::CheckState) => {
                let envelope = raw_envelope(io, cid, peer).await;
                let bytes = command::messaging_layer(command::check_state_response(), &envelope);
                if let Err(e) = io.send_p2p(cid, peer, bytes).await {
                    warn!(target: "citadel", "[CONVERSATIONS] {cid}: could not answer {peer}'s CheckState: {e}");
                }
                true
            }
            Some(Inbound::Message {
                envelope,
                contents,
                timestamp,
            }) => {
                self.incoming(io, cid, peer, envelope, contents, timestamp)
                    .await
            }
            Some(other) => self.revision(io, cid, peer, other).await,
        }
    }

    async fn incoming(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        envelope: Envelope,
        contents: String,
        timestamp: f64,
    ) -> bool {
        let Ok(prefs) = preferences(io, cid).await else {
            return false;
        };
        if !prefs.accept_requests_from_strangers && !io.knows_peer(cid, peer).await {
            // Hidden, as the UI hides it: not stored, not acknowledged to the sender.
            return true;
        }
        let message = ConversationMessage {
            // The transport peer, never the payload's `sender_cid`, which the
            // peer chose: a forged one would render as "You".
            sender_cid: peer,
            recipient_cid: cid,
            status: MessageStatus::Delivered,
            ..received(&envelope, contents, timestamp)
        };
        self.store_and_ack(io, cid, peer, message).await
    }

    /// Store `message` and send the app-level "delivered" (also for a duplicate:
    /// the sender asking again means it never saw the first one).
    async fn store_and_ack(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        message: ConversationMessage,
    ) -> bool {
        let username = io.peer_username(cid, peer);
        let appended = {
            let _held = self.lock(cid, peer).await;
            let s = store(io, cid);
            let appended = s
                .append(peer, &message, username.clone(), io.now_ms())
                .await;
            // A name learned since the conversation began is kept, as the UI's
            // 'p2p:peer-registered' listener kept it.
            if let (Ok(_), Some(name)) = (&appended, &username) {
                if let Err(e) = s.set_peer_username(peer, name, io.now_ms()).await {
                    warn!(target: "citadel", "[CONVERSATIONS] {cid}: peer name not recorded: {e}");
                }
            }
            appended
        };
        match appended {
            Err(e) => {
                warn!(target: "citadel", "[CONVERSATIONS] {cid}: could not store {}: {e}; ILM will deliver it again", message.id);
                false
            }
            Ok(added) => {
                let ack = command::ack(AckKind::Delivered, &message.id, io.now_ms());
                if let Err(e) = io.send_p2p(cid, peer, ack).await {
                    warn!(target: "citadel", "[CONVERSATIONS] {cid}: delivery receipt to {peer} not sent: {e}");
                }
                if let Some(metadata) = added {
                    let mut change = Change::message(ConversationEventKind::Appended, message);
                    change.metadata = Some(metadata);
                    self.announce(io, cid, peer, change).await;
                }
                true
            }
        }
    }

    async fn revision(
        &self,
        io: &dyn ConversationIo,
        cid: u64,
        peer: u64,
        command: Inbound,
    ) -> bool {
        let now = io.now_ms();
        let result = {
            let _held = self.lock(cid, peer).await;
            let s = store(io, cid);
            match command {
                Inbound::Edit {
                    target,
                    contents,
                    edited_at,
                } => s
                    .revise(
                        peer,
                        &target,
                        peer,
                        Revision::Edit {
                            contents,
                            edited_at,
                        },
                        now,
                    )
                    .await
                    .map(|r| applied(r, ConversationEventKind::Updated)),
                Inbound::Delete { target } => s
                    .revise(peer, &target, peer, Revision::Delete, now)
                    .await
                    .map(|r| applied(r, ConversationEventKind::Removed)),
                Inbound::Reaction {
                    target,
                    emoji,
                    active,
                    at,
                } => s
                    .react(
                        peer,
                        &target,
                        &Reaction {
                            emoji,
                            reactor_cid: peer,
                            at,
                            active,
                        },
                    )
                    .await
                    .map(|m| m.map(|m| Change::message(ConversationEventKind::Updated, m))),
                Inbound::Ack { kind, message_id } => s
                    .set_status(peer, &message_id, status_of(kind), None)
                    .await
                    .map(|m| m.map(|m| Change::message(ConversationEventKind::Updated, m))),
                Inbound::Screenshot { envelope, taken_at } => {
                    drop(_held);
                    return self.screenshot(io, cid, peer, envelope, taken_at).await;
                }
                _ => Ok(None),
            }
        };
        match result {
            Ok(Some(change)) => {
                self.announce(io, cid, peer, change).await;
                true
            }
            Ok(None) => true,
            Err(e) => {
                warn!(target: "citadel", "[CONVERSATIONS] {cid}: could not apply a change from {peer}: {e}");
                false
            }
        }
    }
}

fn applied(result: Revised, kind: ConversationEventKind) -> Option<Change> {
    match result {
        Revised::Applied(m) => Some(Change::message(kind, *m)),
        Revised::NotSender | Revised::Unknown => None,
    }
}

fn status_of(kind: AckKind) -> MessageStatus {
    match kind {
        AckKind::Delivered => MessageStatus::Delivered,
        AckKind::Read => MessageStatus::Read,
        AckKind::Failed => MessageStatus::Failed,
    }
}
