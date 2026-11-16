//! `https://<domain>/.well-known/citadel` (`server-identity-challenge.md` §3): the fetch is a
//! port, every decision about what was fetched is `citadel_identity::parse_well_known`.

use async_trait::async_trait;
use citadel_identity::CanonicalDomain;
use std::time::Duration;

/// The contract's cap on the document.
pub const MAX_WELL_KNOWN_BYTES: usize = 64 * 1024;

/// Fetches a server's `/.well-known/citadel` document, unparsed.
#[async_trait]
pub trait WellKnownFetch: Send + Sync + 'static {
    async fn fetch(&self, domain: &CanonicalDomain) -> Result<Vec<u8>, String>;
}

/// Where a canonical domain publishes its document; a path-scoped tenant keeps its path.
pub(crate) fn well_known_url(domain: &CanonicalDomain) -> String {
    format!("https://{domain}/.well-known/citadel")
}

/// HTTPS with the webpki roots the updater uses, `Accept: application/json`, no redirects (a
/// document is only ever trusted from the name it was asked for), the 64 KiB cap enforced while
/// reading, and the caller's timeout for the whole exchange.
pub struct HttpsWellKnownFetch {
    client: reqwest::Client,
}

impl HttpsWellKnownFetch {
    pub fn new(timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|err| err.to_string())?;
        Ok(Self { client })
    }
}

#[async_trait]
impl WellKnownFetch for HttpsWellKnownFetch {
    async fn fetch(&self, domain: &CanonicalDomain) -> Result<Vec<u8>, String> {
        let url = well_known_url(domain);
        let mut response = self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|err| format!("{url}: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("{url}: HTTP {}", response.status()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|err| format!("{url}: {err}"))?
        {
            if body.len() + chunk.len() > MAX_WELL_KNOWN_BYTES {
                return Err(format!("{url}: over {MAX_WELL_KNOWN_BYTES} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_path_is_kept_in_the_url() {
        let d = citadel_identity::canonical_domain("https://Shared.example/Tenant-A/").unwrap();
        assert_eq!(
            well_known_url(&d),
            "https://shared.example/tenant-a/.well-known/citadel"
        );
        let d = citadel_identity::canonical_domain("acme.example:8443").unwrap();
        assert_eq!(
            well_known_url(&d),
            "https://acme.example:8443/.well-known/citadel"
        );
    }
}
