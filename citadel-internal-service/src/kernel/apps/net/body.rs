//! A buffered `AppFetch`: the whole body, within the policy's cap and time.

use super::fetch::Opened;
use super::rate::InstallKey;
use super::{AppNet, FetchInput};
use citadel_app_net_policy::{
    response_header_ok, MAX_HEADERS, MAX_HEADER_BYTES, MAX_RESPONSE_BODY_BYTES,
};
use citadel_internal_service_types::{
    AppFetchLimit, AppFetchRefusal, AppFetchUnavailable, AppHttpHeader,
};
use citadel_sdk::logging::info;
use tokio::time::{timeout_at, Instant};

/// A whole response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fetched {
    pub status: u16,
    pub headers: Vec<AppHttpHeader>,
    pub body: Vec<u8>,
    pub redirects: u8,
}

/// The response headers an app is shown: none the policy denies (cookies, authentication
/// challenges, framing), at most `MAX_HEADERS`, each within `MAX_HEADER_BYTES`; the rest are
/// dropped rather than handed on.
pub(crate) fn shown_headers(headers: &[(String, String)]) -> Vec<AppHttpHeader> {
    headers
        .iter()
        .filter(|(name, value)| {
            response_header_ok(name) && name.len() + value.len() <= MAX_HEADER_BYTES
        })
        .take(MAX_HEADERS)
        .map(|(name, value)| AppHttpHeader {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

impl AppNet {
    pub(crate) async fn fetch(
        &self,
        input: FetchInput<'_>,
        deadline: Instant,
    ) -> Result<Fetched, AppFetchRefusal> {
        let key = input.key;
        let Opened {
            mut upstream,
            redirects,
        } = self.open(input, deadline).await?;
        let mut body = Vec::new();
        loop {
            let chunk = timeout_at(deadline, upstream.body.next_chunk())
                .await
                .map_err(|_| self.limit(key, AppFetchLimit::Time))?
                .map_err(|why| network(key, &why))?;
            let Some(chunk) = chunk else { break };
            self.meter.received(key, chunk.len());
            if (body.len() + chunk.len()) as u64 > MAX_RESPONSE_BODY_BYTES {
                return Err(self.limit(key, AppFetchLimit::ResponseBody));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Fetched {
            status: upstream.head.status,
            headers: shown_headers(&upstream.head.headers),
            body,
            redirects,
        })
    }

    pub(crate) fn limit(&self, key: &InstallKey, what: AppFetchLimit) -> AppFetchRefusal {
        self.meter.refused(key);
        AppFetchRefusal::Limit(what)
    }
}

pub(crate) fn network(key: &InstallKey, why: &str) -> AppFetchRefusal {
    info!(target: "citadel", "[APP-NET] {}: upstream body failed: {why}", key.install_id);
    AppFetchRefusal::Unavailable(AppFetchUnavailable::Network)
}
