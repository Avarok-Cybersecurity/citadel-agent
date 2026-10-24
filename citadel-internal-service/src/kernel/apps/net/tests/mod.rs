//! Host egress end to end over in-process fakes: a resolver that answers what the test says,
//! a connector that records every attempt and opens nothing, and a clock the test sets. No
//! test here touches the network.

mod allowed;
mod denied;
mod fakes;
mod limits;
mod private_network;
mod redirects;
mod streaming;

use fakes::*;

use super::ports::NetPorts;
use super::rate::InstallKey;
use super::{AppNet, AppNetConfig, FetchInput};
use citadel_internal_service_types::{AppHttpHeader, AppHttpRequest, AppNetGrant};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

pub(super) const PAGE: &str = "acme.work.avarok.net";
pub(super) const APP: &str = "com.acme.kanban";

/// An `AppNet` over fakes, and the fakes.
pub(super) struct World {
    pub net: Arc<AppNet>,
    pub resolver: Arc<FakeResolver>,
    pub connector: Arc<FakeConnector>,
    pub clock: Arc<FakeClock>,
    pub settings: FakeSettings,
}

pub(super) fn world_with(agent_port: u16, settings: FakeSettings) -> World {
    let resolver = Arc::new(FakeResolver::default());
    let connector = Arc::new(FakeConnector::default());
    let clock = Arc::new(FakeClock::default());
    let ports = NetPorts {
        resolver: Box::new(resolver.clone()),
        connector: Box::new(connector.clone()),
        clock: Box::new(clock.clone()),
    };
    let config = AppNetConfig {
        page_origin_hosts: vec![PAGE.to_string()],
        agent_port,
    };
    let net = Arc::new(AppNet::new(config, ports).expect("configured"));
    World {
        net,
        resolver,
        connector,
        clock,
        settings,
    }
}

/// The member's private-network grant on: most tests here exercise what the policy itself
/// refuses, which it refuses with the ceiling raised; a member without it is
/// private_network.rs's.
pub(super) fn world_on_port(agent_port: u16) -> World {
    world_with(agent_port, FakeSettings::granting(key().cid, true))
}

pub(super) fn world() -> World {
    world_on_port(citadel_app_net_policy::AGENT_PORT)
}

pub(super) fn key() -> InstallKey {
    InstallKey {
        cid: 42,
        install_id: "inst-1".to_string(),
    }
}

pub(super) fn grant(private: bool, hosts: &[&str]) -> AppNetGrant {
    AppNetGrant {
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        private,
    }
}

pub(super) fn get(url: &str) -> AppHttpRequest {
    AppHttpRequest {
        method: "GET".into(),
        url: url.into(),
        headers: vec![AppHttpHeader {
            name: "accept".into(),
            value: "application/json".into(),
        }],
        body: vec![],
    }
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_millis(citadel_app_net_policy::REQUEST_TIMEOUT_MS)
}

impl World {
    pub(super) async fn fetch(
        &self,
        grant: &AppNetGrant,
        request: &AppHttpRequest,
    ) -> Result<super::body::Fetched, citadel_internal_service_types::AppFetchRefusal> {
        self.fetch_as(APP, grant, request).await
    }

    pub(super) async fn fetch_as(
        &self,
        app_id: &str,
        grant: &AppNetGrant,
        request: &AppHttpRequest,
    ) -> Result<super::body::Fetched, citadel_internal_service_types::AppFetchRefusal> {
        let key = key();
        let input = self.input(&key, app_id, grant, request);
        self.net.fetch(input, deadline()).await
    }

    /// What a fetch is handed, with this world's authorities.
    pub(super) fn input<'a>(
        &'a self,
        key: &'a InstallKey,
        app_id: &'a str,
        grant: &'a AppNetGrant,
        request: &'a AppHttpRequest,
    ) -> FetchInput<'a> {
        FetchInput {
            key,
            app_id,
            grant,
            request,
            member: &self.settings,
        }
    }
}
