//! Conversation requests from a window, answered by the engine.

use super::engine::{store, Change, Engine};
use super::io::ConversationIo;
use super::outbound::Outgoing;
use citadel_internal_service_types::{
    ConversationEventKind, ConversationFailure, ConversationUpdated,
    InternalServiceRequest as Request, InternalServiceResponse as Response,
};
use uuid::Uuid;

impl Engine {
    pub(crate) async fn answer(&self, io: &dyn ConversationIo, request: Request) -> Response {
        let cid = request.session_cid().unwrap_or(0);
        let request_id = request.request_id().copied();
        match self.answer_inner(io, request).await {
            Ok(response) => response,
            Err(message) => Response::ConversationFailure(ConversationFailure {
                cid,
                message,
                request_id,
            }),
        }
    }

    async fn answer_inner(
        &self,
        io: &dyn ConversationIo,
        request: Request,
    ) -> Result<Response, String> {
        let updated = |cid: u64, peer: u64, request_id: Uuid, message| {
            Response::ConversationUpdated(Box::new(ConversationUpdated {
                cid,
                peer_cid: peer,
                message,
                request_id: Some(request_id),
            }))
        };
        Ok(match request {
            Request::ConversationSend {
                request_id,
                cid,
                peer_cid,
                content,
                message_type,
                reply_to,
                mentions,
                attachments,
                document_id,
                document_title,
                security_level: _,
            } => {
                let out = Outgoing {
                    content,
                    message_type,
                    reply_to,
                    mentions,
                    attachments,
                    document_id,
                    document_title,
                };
                updated(
                    cid,
                    peer_cid,
                    request_id,
                    self.send(io, cid, peer_cid, out).await?,
                )
            }
            Request::ConversationResend {
                request_id,
                cid,
                peer_cid,
                message_id,
            } => updated(
                cid,
                peer_cid,
                request_id,
                self.resend(io, cid, peer_cid, &message_id).await?,
            ),
            Request::ConversationEdit {
                request_id,
                cid,
                peer_cid,
                message_id,
                contents,
            } => updated(
                cid,
                peer_cid,
                request_id,
                self.revise_own(io, cid, peer_cid, &message_id, Some(contents))
                    .await?,
            ),
            Request::ConversationDelete {
                request_id,
                cid,
                peer_cid,
                message_id,
            } => updated(
                cid,
                peer_cid,
                request_id,
                self.revise_own(io, cid, peer_cid, &message_id, None)
                    .await?,
            ),
            Request::ConversationReact {
                request_id,
                cid,
                peer_cid,
                message_id,
                emoji,
                active,
            } => updated(
                cid,
                peer_cid,
                request_id,
                self.react(io, cid, peer_cid, &message_id, emoji, active)
                    .await?,
            ),
            Request::ConversationMarkRead {
                request_id,
                cid,
                peer_cid,
            } => updated(
                cid,
                peer_cid,
                request_id,
                self.mark_read(io, cid, peer_cid).await?,
            ),
            Request::ConversationRecord {
                request_id,
                cid,
                peer_cid,
                message,
            } => {
                let parties = (message.sender_cid, message.recipient_cid);
                if parties != (cid, peer_cid) && parties != (peer_cid, cid) {
                    return Err("A record must be between this account and the peer".to_string());
                }
                let _held = self.lock(cid, peer_cid).await;
                let message = *message;
                let added = store(io, cid)
                    .append(
                        peer_cid,
                        &message,
                        io.peer_username(cid, peer_cid),
                        io.now_ms(),
                    )
                    .await?;
                if let Some(metadata) = added {
                    let mut change =
                        Change::message(ConversationEventKind::Appended, message.clone());
                    change.metadata = Some(metadata);
                    self.announce(io, cid, peer_cid, change).await;
                }
                updated(cid, peer_cid, request_id, Some(message))
            }
            Request::ConversationPatch {
                request_id,
                cid,
                peer_cid,
                message_id,
                patch,
            } => {
                let _held = self.lock(cid, peer_cid).await;
                let changed = store(io, cid).patch(peer_cid, &message_id, *patch).await?;
                if let Some(m) = &changed {
                    self.announce(
                        io,
                        cid,
                        peer_cid,
                        Change::message(ConversationEventKind::Updated, m.clone()),
                    )
                    .await;
                }
                updated(cid, peer_cid, request_id, changed)
            }
            Request::ConversationClear {
                request_id,
                cid,
                peer_cid,
                include_unattributed,
            } => {
                let _held = self.lock(cid, peer_cid).await;
                if !store(io, cid)
                    .delete(peer_cid, include_unattributed)
                    .await?
                {
                    return Err("This conversation is not this account's to clear".to_string());
                }
                let change = Change {
                    kind: ConversationEventKind::Cleared,
                    message: None,
                    message_id: None,
                    metadata: None,
                };
                self.announce(io, cid, peer_cid, change).await;
                updated(cid, peer_cid, request_id, None)
            }
            read @ (Request::ConversationList { .. }
            | Request::ConversationPage { .. }
            | Request::SetAccountPreferences { .. }
            | Request::GetAccountPreferences { .. }) => self.answer_read(io, read).await?,
            other => {
                return Err(format!(
                    "not a conversation request: {:?}",
                    other.request_id()
                ))
            }
        })
    }
}
