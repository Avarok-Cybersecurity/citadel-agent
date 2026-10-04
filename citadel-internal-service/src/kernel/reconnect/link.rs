//! Putting a newly opened link into a session's entry: one path for the reconnect and for
//! a sign-in that takes a reconnect over, so both leave the entry, its reader and the UI in
//! the same state.

use super::report::{logged, notify};
use super::{LinkState, LOG_TARGET};
use crate::kernel::session_route::SessionRoute;
use crate::kernel::{c2s_reader, create_client_server_remote, CitadelWorkspaceService, Connection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{InternalServiceResponse, ServerReconnected};
use citadel_sdk::logging::info;
use citadel_sdk::prelude::{
    CitadelClientServerConnection, ProtocolRemoteTargetExt, Ratchet, SessionSecuritySettings,
};
use std::sync::Arc;
use uuid::Uuid;

/// Install `connected` into `cid`'s entry if `admit` accepts it, under the map lock;
/// `admit` may also update the entry (a sign-in re-points its owner). The entry keeps its
/// subscribers and its groups (the server prompts the rejoins into this same entry);
/// peers were cleared when the link dropped. Returns whether it was installed: a link
/// nobody admits is closed, since the session it opened has no owner.
pub(super) async fn put_link<T: IOInterface + Sync, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    cid: u64,
    connected: CitadelClientServerConnection<R>,
    settings: SessionSecuritySettings,
    connect_request_id: Uuid,
    admit: impl FnOnce(&mut Connection<R>) -> bool,
) -> bool {
    let (sink, stream, handle) = match crate::kernel::sign_in::open(connected) {
        Ok(opened) => opened,
        Err(err) => {
            logged(cid, "opening the new link", Err(err));
            return false;
        }
    };
    let remote = create_client_server_remote(stream.vconn_type, this.remote().clone(), settings);
    let installed = {
        let mut lock = this.server_connection_map.write();
        match lock.get_mut(&cid) {
            Some(conn) => admit(conn).then(|| {
                conn.sink_to_server = Arc::new(tokio::sync::Mutex::new(sink));
                conn.client_server_remote = remote.clone();
                conn.sign_in.handle = handle;
                conn.link = LinkState::Up;
                conn.subscribers.clone()
            }),
            None => None,
        }
    };
    let Some(subscribers) = installed else {
        info!(target: LOG_TARGET, "[Reconnect] {cid}: nobody wants the new link; closing it");
        logged(cid, "closing an unowned link", remote.disconnect().await);
        return false;
    };
    c2s_reader::spawn(
        SessionRoute::new(subscribers, this.tx_to_localhost_clients.clone()),
        cid,
        stream,
        connect_request_id,
    );
    let sent = notify(
        this,
        cid,
        InternalServiceResponse::ServerReconnected(ServerReconnected {
            cid,
            request_id: None,
        }),
    );
    logged(cid, "sending ServerReconnected", sent);
    true
}
