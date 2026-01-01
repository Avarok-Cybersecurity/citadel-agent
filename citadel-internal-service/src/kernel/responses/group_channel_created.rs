use crate::kernel::{
    requests, send_response_to_tcp_client, CitadelWorkspaceService, GroupConnection,
};
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::{GroupChannelCreateSuccess, InternalServiceResponse};
use citadel_sdk::prelude::{GroupChannelCreated, NetworkError, Ratchet};
use std::sync::atomic::Ordering;

pub async fn handle<T: IOInterface, R: Ratchet>(
    this: &CitadelWorkspaceService<T, R>,
    group_channel_created: GroupChannelCreated,
) -> Result<(), NetworkError> {
    let channel = group_channel_created.channel;
    let cid = channel.cid();
    let key = channel.key();
    let (tx, rx) = channel.split();

    let mut server_connection_map = this.server_connection_map.write();
    if let Some(connection) = server_connection_map.get_mut(&cid) {
        connection.add_group_channel(key, GroupConnection { key, tx, cid });

        let uuid = connection.associated_tcp_connection.load(Ordering::Relaxed);
        requests::spawn_group_channel_receiver(key, cid, uuid, rx, this.tcp_connection_map.clone());

        let associated_tcp_connection =
            connection.associated_tcp_connection.load(Ordering::Relaxed);
        drop(server_connection_map);
        send_response_to_tcp_client(
            &this.tcp_connection_map,
            InternalServiceResponse::GroupChannelCreateSuccess(GroupChannelCreateSuccess {
                cid,
                group_key: key,
                request_id: None,
            }),
            associated_tcp_connection,
        )?;

        Ok(())
    } else {
        Err(NetworkError::Generic(format!(
            "No connection found for cid in connection map: {cid}"
        )))
    }
}
