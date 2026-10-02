use crate::kernel::session_route::SessionRoute;
use crate::kernel::{requests, CitadelWorkspaceService, GroupConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{GroupChannelCreateSuccess, InternalServiceResponse};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{GroupChannel, GroupChannelCreated, NetworkError, Ratchet};
use futures::StreamExt;

/// Adopts a group channel the SDK opened on its own: a joined group after a member's
/// rejoin, or an owned one the owner re-founded after reconnecting. Both arrive the same
/// way, and both go into `Connection.groups` like a channel this session asked for.
pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    group_channel_created: GroupChannelCreated,
) -> Result<(), NetworkError> {
    let channel = group_channel_created.channel;
    let cid = channel.cid();
    let key = channel.key();

    let mut server_connection_map = this.server_connection_map.write();
    if let Some(connection) = server_connection_map.get_mut(&cid) {
        let (tx, rx) = channel.split();
        connection.add_group_channel(key, GroupConnection { key, tx, cid });

        let route = SessionRoute::new(
            connection.subscribers.clone(),
            this.tx_to_localhost_clients.clone(),
        );
        let departed = connection.groups.departure_flag(&key);
        requests::spawn_group_channel_receiver(key, cid, route.clone(), departed, rx);

        drop(server_connection_map);
        route.send(InternalServiceResponse::GroupChannelCreateSuccess(
            GroupChannelCreateSuccess {
                cid,
                group_key: key,
                request_id: None,
            },
        ));

        Ok(())
    } else {
        drop(server_connection_map);
        park(channel);
        Err(NetworkError::generic(format!(
            "No connection found for cid in connection map: {cid}"
        )))
    }
}

/// Keeps a channel no session adopted until its SDK session ends.
///
/// Dropping a `GroupChannelRecvHalf` sends `LeaveRoom`, which on a live session removes
/// the member (and for an owner, its group) for real. A channel nobody can use is still
/// membership the user did not give up, so it is held, and dropped only once the session
/// that could send that `LeaveRoom` is gone.
pub(crate) fn park(channel: GroupChannel) {
    let key = channel.key();
    let cid = channel.cid();
    warn!(target: "citadel", "[GroupChannelCreated] no session {cid} to adopt the channel for {key:?}; holding it until the session ends");
    let (_tx, mut rx) = channel.split();
    drop(tokio::spawn(
        async move { while rx.next().await.is_some() {} },
    ));
}
