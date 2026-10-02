//! Reading conversations and preferences, and saving preferences.

use super::engine::{preferences, save_preferences, store, Engine};
use super::io::ConversationIo;
use citadel_internal_service_types::{
    AccountPreferencesResponse, ConversationListResponse, ConversationPageResponse,
    InternalServiceRequest as Request, InternalServiceResponse as Response,
};

impl Engine {
    pub(super) async fn answer_read(
        &self,
        io: &dyn ConversationIo,
        request: Request,
    ) -> Result<Response, String> {
        Ok(match request {
            Request::ConversationList { request_id, cid } => {
                Response::ConversationListResponse(Box::new(ConversationListResponse {
                    cid,
                    conversations: store(io, cid).list().await?,
                    request_id: Some(request_id),
                }))
            }
            Request::ConversationPage {
                request_id,
                cid,
                peer_cid,
                page,
            } => {
                let s = store(io, cid);
                let metadata = s.load_metadata(peer_cid).await?;
                let number = page.or(metadata.as_ref().map(|m| m.latest_page));
                let page = match number {
                    Some(n) => s.load_page(peer_cid, n).await?,
                    None => None,
                };
                Response::ConversationPageResponse(Box::new(ConversationPageResponse {
                    cid,
                    peer_cid,
                    metadata,
                    page,
                    request_id: Some(request_id),
                }))
            }
            Request::SetAccountPreferences {
                request_id,
                cid,
                preferences,
            } => {
                let preferences = *preferences;
                save_preferences(io, cid, &preferences).await?;
                self.sweep(io, cid).await;
                Response::AccountPreferencesResponse(Box::new(AccountPreferencesResponse {
                    cid,
                    preferences,
                    request_id: Some(request_id),
                }))
            }
            Request::GetAccountPreferences { request_id, cid } => {
                Response::AccountPreferencesResponse(Box::new(AccountPreferencesResponse {
                    cid,
                    preferences: preferences(io, cid).await?,
                    request_id: Some(request_id),
                }))
            }
            other => return Err(format!("not a read request: {:?}", other.request_id())),
        })
    }
}
