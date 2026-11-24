//! The workspace server's grant question and answer as this test defines them (mod.rs).

use super::APP_ID;
use citadel_internal_service::{AuthoritySignal, HostNetAuthority, KernelGrant, SessionLimits};
use serde_json::{json, Value};

pub(super) struct JsonAuthority;

impl HostNetAuthority for JsonAuthority {
    fn grant_query(&self, install_id: &str, tag: u64) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&json!({"q": [install_id, tag]})).map_err(|e| e.to_string())
    }

    fn classify(&self, frame: &[u8]) -> AuthoritySignal {
        let Ok(value) = serde_json::from_slice::<Value>(frame) else {
            return AuthoritySignal::Unrelated;
        };
        let grant = &value["grant"];
        let (Some(install_id), Some(tag)) = (grant["install_id"].as_str(), grant["tag"].as_u64())
        else {
            return AuthoritySignal::Unrelated;
        };
        let servers = grant["servers"].as_array().into_iter().flatten();
        AuthoritySignal::Grant {
            install_id: install_id.to_string(),
            tag,
            grant: Some(KernelGrant {
                app_id: APP_ID.to_string(),
                hosts: Vec::new(),
                private: false,
                servers: servers
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
                session: Some(SessionLimits {
                    max_msg_bytes: 4096,
                    max_msgs_per_s: 8,
                }),
            }),
        }
    }
}
