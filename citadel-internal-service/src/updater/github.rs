//! `ReleaseSource` over HTTPS to GitHub. Every request, and every redirect it follows, must pass
//! `release::url_allowed`; nothing else is reachable from here.

use super::io::{Fetched, ReleaseSource};
use super::release::{url_allowed, LATEST_URL, REPO};
use async_trait::async_trait;
use reqwest::header::{ACCEPT, ETAG, IF_NONE_MATCH, USER_AGENT};
use reqwest::{redirect, Client, StatusCode, Url};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Between two reads of a download, not the whole of it: a slow line still finishes.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;

pub struct GitHub {
    client: Client,
    user_agent: String,
}

impl GitHub {
    /// `version` names this agent in the User-Agent GitHub requires.
    pub fn new(version: &str) -> Result<Self, String> {
        let policy = redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            match url_allowed(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(why) => attempt.error(why),
            }
        });
        let client = Client::builder()
            .https_only(true)
            .redirect(policy)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(|e| format!("the HTTP client did not start: {e}"))?;
        Ok(Self {
            client,
            user_agent: format!("citadel-agent/{version}"),
        })
    }

    async fn get(
        &self,
        url: &str,
        accept: &str,
        etag: Option<String>,
    ) -> Result<reqwest::Response, String> {
        let parsed = Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
        url_allowed(&parsed)?;
        let mut request = self
            .client
            .get(parsed)
            .header(USER_AGENT, &self.user_agent)
            .header(ACCEPT, accept);
        if let Some(etag) = etag {
            request = request.header(IF_NONE_MATCH, etag);
        }
        request.send().await.map_err(|e| format!("{url}: {e}"))
    }
}

/// The body, refused once it passes `max` bytes, however it is framed.
async fn bounded(mut response: reqwest::Response, max: u64) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if body.len() as u64 + chunk.len() as u64 > max {
            return Err(format!("the response is over {max} bytes"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn ok(response: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else {
        Err(format!("{} answered {status}", response.url()))
    }
}

#[async_trait]
impl ReleaseSource for GitHub {
    async fn latest(&self, etag: Option<String>) -> Result<Fetched, String> {
        let response = self
            .get(LATEST_URL, "application/vnd.github+json", etag)
            .await?;
        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(Fetched::NotModified);
        }
        let response = ok(response)?;
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = bounded(response, MAX_JSON_BYTES).await?;
        Ok(Fetched::Release { body, etag })
    }

    async fn text(&self, url: &str, max_bytes: u64) -> Result<String, String> {
        let body = bounded(
            ok(self.get(url, "application/octet-stream", None).await?)?,
            max_bytes,
        )
        .await?;
        String::from_utf8(body).map_err(|_| format!("{url} is not text"))
    }

    async fn download(&self, url: &str, dest: &Path, size: u64) -> Result<(), String> {
        let mut response = ok(self.get(url, "application/octet-stream", None).await?)?;
        let mut file =
            std::fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let mut written: u64 = 0;
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("{url}: {e}"))? {
            written += chunk.len() as u64;
            if written > size {
                return Err(format!(
                    "{url} is longer than the {size} bytes the release lists"
                ));
            }
            file.write_all(&chunk)
                .map_err(|e| format!("{}: {e}", dest.display()))?;
        }
        file.sync_all()
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        if written != size {
            return Err(format!("{url} ended at {written} of {size} bytes"));
        }
        Ok(())
    }

    async fn attestations(&self, sha256: &str) -> Result<Vec<String>, String> {
        let url = format!("https://api.github.com/repos/{REPO}/attestations/sha256:{sha256}");
        let response = self.get(&url, "application/vnd.github+json", None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let body = bounded(ok(response)?, MAX_JSON_BYTES).await?;
        let json: serde_json::Value =
            serde_json::from_slice(&body).map_err(|e| format!("attestations: {e}"))?;
        Ok(json["attestations"]
            .as_array()
            .map(|all| all.iter().map(|a| a["bundle"].to_string()).collect())
            .unwrap_or_default())
    }
}
