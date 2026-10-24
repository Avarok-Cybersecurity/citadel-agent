//! A member's private-network grant for apps, on a real agent signed in to a real server:
//! the session's own window sets it with the owned request, a raw LocalDB write cannot touch
//! it (nor the updater's auto-install choice), and a window not attached to the session is
//! refused.

use citadel_internal_service_test_common as common;

use citadel_internal_service_types::{InternalServiceRequest, InternalServiceResponse};
use citadel_sdk::prelude::StackedRatchet;
use common::group::recv_until;
use std::error::Error;
use std::net::SocketAddr;
use uuid::Uuid;

const GRANT_KEY: &str = "agent:apps_private_network";
const AUTO_INSTALL_KEY: &str = "agent_update_auto_install";
const REFUSED: &str = "agent: keys are the agent's own settings";

fn set(cid: u64, on: bool) -> InternalServiceRequest {
    InternalServiceRequest::AppsSetAgentSettings {
        request_id: Uuid::new_v4(),
        cid,
        private_network: on,
    }
}

fn get(cid: u64) -> InternalServiceRequest {
    InternalServiceRequest::AppsGetAgentSettings {
        request_id: Uuid::new_v4(),
        cid,
    }
}

fn raw_set(cid: u64, key: &str) -> InternalServiceRequest {
    InternalServiceRequest::LocalDBSetKV {
        request_id: Uuid::new_v4(),
        cid,
        peer_cid: None,
        key: key.to_string(),
        value: vec![0],
    }
}

fn settings_answer(r: &InternalServiceResponse) -> bool {
    matches!(
        r,
        InternalServiceResponse::AppsAgentSettings(_)
            | InternalServiceResponse::AppsAgentSettingsFailure(_)
    )
}

fn set_kv_answer(r: &InternalServiceResponse) -> bool {
    matches!(
        r,
        InternalServiceResponse::LocalDBSetKVSuccess(_)
            | InternalServiceResponse::LocalDBSetKVFailure(_)
    )
}

fn granted(r: &InternalServiceResponse) -> Option<bool> {
    match r {
        InternalServiceResponse::AppsAgentSettings(s) => Some(s.private_network),
        _ => None,
    }
}

fn refusal(r: &InternalServiceResponse) -> Option<&str> {
    match r {
        InternalServiceResponse::LocalDBSetKVFailure(f) => Some(&f.message),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_members_grant_is_set_by_their_window_and_by_nothing_else() -> Result<(), Box<dyn Error>>
{
    common::setup_log();
    let addr: SocketAddr = format!("127.0.0.1:{}", common::get_free_port()).parse()?;
    let mut sessions =
        common::services_connected_to_one_server::<StackedRatchet>(vec![addr], None).await?;
    let (tx, mut rx, cid) = sessions.remove(0);

    tx.send(get(cid))?;
    let unset = recv_until(&mut rx, "unset grant", settings_answer).await;
    assert_eq!(granted(&unset), Some(false), "{unset:?}");

    tx.send(set(cid, true))?;
    let on = recv_until(&mut rx, "grant on", settings_answer).await;
    assert_eq!(granted(&on), Some(true), "{on:?}");

    // The session's own window, writing the key raw, is refused; so is the updater's key.
    for (bucket, key) in [
        (cid, GRANT_KEY),
        (0, AUTO_INSTALL_KEY),
        (cid, AUTO_INSTALL_KEY),
    ] {
        tx.send(raw_set(bucket, key))?;
        let raw = recv_until(&mut rx, "raw write", set_kv_answer).await;
        assert_eq!(refusal(&raw), Some(REFUSED), "{key} in {bucket}: {raw:?}");
    }
    // The control: an ordinary key in the same bucket is written.
    tx.send(raw_set(cid, "ui-pref"))?;
    let ordinary = recv_until(&mut rx, "ordinary write", set_kv_answer).await;
    assert!(
        matches!(ordinary, InternalServiceResponse::LocalDBSetKVSuccess(_)),
        "{ordinary:?}"
    );

    // A window not attached to the session cannot turn it off.
    let (stranger, mut stranger_rx) = common::open_localhost_connection(addr).await?;
    stranger.send(set(cid, false))?;
    let refused = recv_until(&mut stranger_rx, "stranger's set", settings_answer).await;
    assert!(
        matches!(
            refused,
            InternalServiceResponse::AppsAgentSettingsFailure(_)
        ),
        "{refused:?}"
    );

    tx.send(get(cid))?;
    let still = recv_until(&mut rx, "grant after both attempts", settings_answer).await;
    assert_eq!(granted(&still), Some(true), "{still:?}");
    Ok(())
}
