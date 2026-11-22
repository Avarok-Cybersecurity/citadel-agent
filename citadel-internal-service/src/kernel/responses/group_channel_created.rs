use crate::kernel::session_route::SessionRoute;
use crate::kernel::{requests, CitadelWorkspaceService, GroupConnection};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{GroupChannelCreateSuccess, InternalServiceResponse};
use citadel_sdk::logging::warn;
use citadel_sdk::prelude::{GroupChannel, GroupChannelCreated, NetworkError, Ratchet};

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
        let state = connection.groups.channel_state(&key);
        requests::spawn_group_channel_receiver(key, cid, route.clone(), state, rx);

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
        release(channel);
        Err(NetworkError::generic(format!(
            "No connection found for cid in connection map: {cid}"
        )))
    }
}

/// Lets go of a channel no session adopted. Dropping it sends nothing: the member stays in the
/// group (as its owner, if it was), the group's traffic reaches the kernel as `GroupEvent`s
/// (`responses/group_heard.rs`), and the next sign-in's restore opens a new channel.
pub(crate) fn release(channel: GroupChannel) {
    warn!(target: "citadel", "[GroupChannelCreated] no session {} to adopt the channel for {:?}; the membership is kept", channel.cid(), channel.key());
    drop(channel);
}
