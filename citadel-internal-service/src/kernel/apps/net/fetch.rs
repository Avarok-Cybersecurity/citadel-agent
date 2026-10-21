//! One request, decided, resolved, decided again, pinned and opened; redirects likewise.

use super::ports::{PinnedRequest, Upstream};
use super::{AppNet, FetchInput};
use citadel_app_net_policy::{
    decide, decide_redirect, Allowed, DenyReason, Egress, EgressRequest, NetGrant, Scheme,
    TargetUrl, Verdict,
};
use citadel_internal_service_types::{AppFetchLimit, AppFetchRefusal, AppFetchUnavailable};
use citadel_sdk::logging::info;
use std::net::IpAddr;
use tokio::time::{timeout_at, Instant};

/// Statuses whose `location` is followed (and decided again).
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

/// An upstream response that is not a redirect, and how many redirects led to it.
pub(crate) struct Opened {
    pub upstream: Upstream,
    pub redirects: u8,
}

fn policy(install: &str, reason: &DenyReason) -> AppFetchRefusal {
    // For the operator; the app is told `Policy` and nothing else.
    info!(target: "citadel", "[APP-NET] {install}: refused by policy: {reason:?}");
    AppFetchRefusal::Policy
}

/// The URL actually requested: rebuilt from the checked target, so the connector is handed
/// exactly what the policy read.
fn target_url(target: &TargetUrl) -> String {
    let scheme = match target.scheme {
        Scheme::Https => "https",
        Scheme::Http => "http",
    };
    format!(
        "{scheme}://{}:{}{}",
        target.host, target.port, target.path_and_query
    )
}

/// One hop: what is asked, and the hop it follows, if any.
struct Hop<'a> {
    method: &'a str,
    url: &'a str,
    headers: &'a [(String, String)],
    body_len: u64,
    previous: Option<&'a Allowed>,
    number: u8,
}

impl AppNet {
    /// The request's upstream response, or why there is none. Every policy refusal happens
    /// before the connector is asked for anything.
    pub(crate) async fn open(
        &self,
        input: FetchInput<'_>,
        deadline: Instant,
    ) -> Result<Opened, AppFetchRefusal> {
        self.meter.request(input.key);
        let opened = self.open_hops(&input, deadline).await;
        if let Err(AppFetchRefusal::Policy | AppFetchRefusal::Limit(_)) = &opened {
            self.meter.refused(input.key);
        }
        opened
    }

    async fn open_hops(
        &self,
        input: &FetchInput<'_>,
        deadline: Instant,
    ) -> Result<Opened, AppFetchRefusal> {
        let install = input.key.install_id.as_str();
        if !self.rate.admit(input.key, self.ports.clock.now_ms()) {
            return Err(AppFetchRefusal::Limit(AppFetchLimit::Rate));
        }
        let hosts: Vec<&str> = input.grant.hosts.iter().map(String::as_str).collect();
        let egress = Egress::Host {
            private: input.grant.private,
        };
        let grant = NetGrant::new(egress, &hosts).map_err(|fault| {
            info!(target: "citadel", "[APP-NET] {install}: refused grant: {fault:?}");
            AppFetchRefusal::Policy
        })?;
        let headers: Vec<(String, String)> = input
            .request
            .headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect();
        let mut method = input.request.method.clone();
        let mut body = input.request.body.clone();
        let mut url = input.request.url.clone();
        let mut previous: Option<Allowed> = None;
        let mut number: u8 = 0;
        loop {
            let hop = Hop {
                method: &method,
                url: &url,
                headers: &headers,
                body_len: body.len() as u64,
                previous: previous.as_ref(),
                number,
            };
            let allowed = self.check(install, &hop, &grant, deadline).await?;
            let pinned = PinnedRequest {
                method: method.clone(),
                url: target_url(&allowed.target),
                host: allowed.target.host.clone(),
                connect: allowed.connect.clone(),
                headers: headers.clone(),
                body: body.clone(),
            };
            self.meter.sent(input.key, body.len());
            let upstream = timeout_at(deadline, self.ports.connector.open(pinned))
                .await
                .map_err(|_| AppFetchRefusal::Limit(AppFetchLimit::Time))?
                .map_err(|why| {
                    info!(target: "citadel", "[APP-NET] {install}: upstream failed: {why}");
                    AppFetchRefusal::Unavailable(AppFetchUnavailable::Network)
                })?;
            let status = upstream.head.status;
            let location = REDIRECT_STATUSES
                .contains(&status)
                .then(|| upstream.head.header("location").map(str::to_string))
                .flatten();
            let Some(location) = location else {
                return Ok(Opened {
                    upstream,
                    redirects: number,
                });
            };
            // The redirect's own body is never read: the connection is dropped here.
            drop(upstream);
            url = allowed.follow(&location);
            if status == 303 || (matches!(status, 301 | 302) && method == "POST") {
                method = "GET".to_string();
                body.clear();
            }
            previous = Some(allowed);
            number = number.saturating_add(1);
        }
    }

    /// The policy over every deployment the agent serves: allowed only if all allow.
    fn decide_all(&self, hop: &Hop<'_>, resolved: &[IpAddr], grant: &NetGrant) -> Verdict {
        let request = EgressRequest {
            method: hop.method,
            url: hop.url,
            headers: hop.headers,
            body_len: hop.body_len,
        };
        let mut allowed = None;
        for deployment in &self.deployments {
            let verdict = match hop.previous {
                None => decide(&request, resolved, grant, deployment),
                Some(previous) => {
                    decide_redirect(previous, hop.number, &request, resolved, grant, deployment)
                }
            };
            match verdict {
                Verdict::Allow(a) => allowed = Some(a),
                refused => return refused,
            }
        }
        match allowed {
            Some(a) => Verdict::Allow(a),
            // `AppNet::new` refuses an empty deployment list.
            None => Verdict::Policy(DenyReason::NoAddress),
        }
    }

    /// Decide before resolving (a name the grant does not admit is never even looked up),
    /// then resolve and decide every address.
    async fn check(
        &self,
        install: &str,
        hop: &Hop<'_>,
        grant: &NetGrant,
        deadline: Instant,
    ) -> Result<Allowed, AppFetchRefusal> {
        let allowed = match self.decide_all(hop, &[], grant) {
            Verdict::Allow(allowed) => allowed,
            Verdict::Policy(DenyReason::NoAddress) => {
                let target = TargetUrl::parse(hop.url)
                    .map_err(|fault| policy(install, &DenyReason::Url(fault)))?;
                let lookup = self.ports.resolver.resolve(&target.host, target.port);
                let resolved = timeout_at(deadline, lookup)
                    .await
                    .map_err(|_| AppFetchRefusal::Limit(AppFetchLimit::Time))?
                    .unwrap_or_default();
                match self.decide_all(hop, &resolved, grant) {
                    Verdict::Allow(allowed) => allowed,
                    Verdict::Policy(reason) => return Err(policy(install, &reason)),
                }
            }
            Verdict::Policy(reason) => return Err(policy(install, &reason)),
        };
        if allowed.target.port == self.agent_port {
            return Err(policy(install, &DenyReason::AgentPort));
        }
        Ok(allowed)
    }
}
