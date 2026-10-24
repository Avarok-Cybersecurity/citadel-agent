//! What one fetch may reach: the workspace server's word on the install, narrowed by what the
//! page asks, and private addresses only when the member has also allowed them.
//!
//! - The server decides the app: a page naming another app is refused.
//! - The page may narrow the server's hosts, never widen them.
//! - Private addresses need the page to ask, the server to allow it (the installer still holds
//!   the right to grant it), and this member's own grant on this agent.
//! - No answer from the server is no fetch: `Unavailable(Authority)`.

use super::{AppNet, FetchInput};
use citadel_internal_service_types::{AppFetchRefusal, AppFetchUnavailable};
use citadel_sdk::logging::info;

/// What the fetch may reach, as decided.
pub(super) struct Reach {
    pub app_id: String,
    pub hosts: Vec<String>,
    pub private: bool,
}

/// The page's hosts that the server's list names too.
fn narrowed(page: &[String], server: &[String]) -> Vec<String> {
    page.iter()
        .filter(|asked| {
            server
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(asked))
        })
        .cloned()
        .collect()
}

impl AppNet {
    pub(super) async fn authorize(&self, input: &FetchInput<'_>) -> Result<Reach, AppFetchRefusal> {
        let (cid, install) = (input.key.cid, input.key.install_id.as_str());
        let answer = self.grants.resolve(cid, install, input.link).await;
        let server = match answer {
            Ok(Some(server)) => server,
            Ok(None) => {
                info!(target: "citadel", "[APP-NET] {install}: the server allows it nothing");
                return Err(AppFetchRefusal::Policy);
            }
            Err(fault) => {
                info!(target: "citadel", "[APP-NET] {install}: no word from the server: {fault:?}");
                return Err(AppFetchRefusal::Unavailable(AppFetchUnavailable::Authority));
            }
        };
        if server.app_id != input.app_id {
            info!(target: "citadel", "[APP-NET] {install}: the page names {}, the server {}", input.app_id, server.app_id);
            return Err(AppFetchRefusal::Policy);
        }
        let private = input.grant.private && server.private && member_allows_private(input).await;
        Ok(Reach {
            hosts: narrowed(&input.grant.hosts, &server.hosts),
            app_id: server.app_id,
            private,
        })
    }
}

/// Whether this member has let apps reach their private network through this agent. Unset
/// is no, and so is a grant that cannot be read.
async fn member_allows_private(input: &FetchInput<'_>) -> bool {
    let cid = input.key.cid;
    match input.member.private_network(cid).await {
        Ok(granted) => granted == Some(true),
        Err(why) => {
            info!(target: "citadel", "[APP-NET] {cid}: the private-network grant is unreadable: {why}");
            false
        }
    }
}
