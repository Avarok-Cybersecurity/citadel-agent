use crate::kernel::ext::IOInterfaceExt;
use crate::kernel::requests::{handle_request, HandledRequestResult};
use citadel_internal_service_connector::connector::{
    InternalServiceConnector, WrappedSink, WrappedStream,
};
use citadel_internal_service_connector::io_interface::in_memory::{
    InMemoryInterface, InMemorySink, InMemoryStream,
};
use citadel_internal_service_connector::io_interface::tcp::TcpIOInterface;
#[cfg(feature = "websockets")]
use citadel_internal_service_connector::io_interface::websockets::WebSocketInterface;
use citadel_internal_service_connector::io_interface::IOInterface;
use citadel_internal_service_types::*;
use citadel_sdk::logging::{error, info, warn};
use citadel_sdk::prefabs::ClientServerRemote;
use citadel_sdk::prelude::remote_specialization::PeerRemote;
use citadel_sdk::prelude::VirtualTargetType;
use citadel_sdk::prelude::*;
use futures::stream::StreamExt;
use futures::{Sink, SinkExt};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

pub(crate) mod ext;
pub(crate) mod requests;
pub(crate) mod responses;

pub type RatchetType = StackedRatchet;

pub struct CitadelWorkspaceService<T, R: Ratchet> {
    pub remote: Option<NodeRemote<R>>,
    /// Session connection map - use .read() for lookups, .write() for insert/remove
    /// CRITICAL: Never hold lock across .await points - use block pattern to extract needed data
    pub server_connection_map: Arc<RwLock<HashMap<u64, Connection<R>>>>,
    /// tx_to_localhost_clients was formerly "tcp_connection_map". Still the same functionality, but more accurately
    /// represents that this map is used for sending messages to localhost clients who COULD be TCP, but NOT necessarily TCP
    /// like websocket clients for browser clients.
    pub tx_to_localhost_clients:
        Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    pub orphan_sessions: Arc<RwLock<HashMap<Uuid, bool>>>, // Maps TCP connection ID to orphan mode
    /// Stores pending PeerConnect signals awaiting UI acceptance.
    /// Key is (session_cid, peer_cid), value is the original PeerSignal for responding.
    pub pending_peer_connect_signals: Arc<RwLock<HashMap<(u64, u64), PeerSignal>>>,
    /// Cache for peer usernames received from registration events.
    /// Key is (session_cid, peer_cid), value is the peer's username.
    /// Used as fallback when SDK's get_local_group_mutual_peers returns empty username.
    pub peer_username_cache: Arc<RwLock<HashMap<(u64, u64), String>>>,
    /// Tracks usernames currently being connected to prevent duplicate concurrent connection attempts.
    /// This prevents TOCTOU race conditions where two Connect requests arrive simultaneously.
    pub connecting_usernames: Arc<Mutex<HashSet<String>>>,
    io: Arc<RwLock<Option<T>>>,
}

impl<T, R: Ratchet> Clone for CitadelWorkspaceService<T, R> {
    fn clone(&self) -> Self {
        CitadelWorkspaceService {
            remote: self.remote.clone(),
            server_connection_map: self.server_connection_map.clone(),
            tx_to_localhost_clients: self.tx_to_localhost_clients.clone(),
            orphan_sessions: self.orphan_sessions.clone(),
            pending_peer_connect_signals: self.pending_peer_connect_signals.clone(),
            peer_username_cache: self.peer_username_cache.clone(),
            connecting_usernames: self.connecting_usernames.clone(),
            io: self.io.clone(),
        }
    }
}

impl<T: IOInterface, R: Ratchet> From<T> for CitadelWorkspaceService<T, R> {
    fn from(io: T) -> Self {
        CitadelWorkspaceService {
            remote: None,
            server_connection_map: Arc::new(RwLock::new(Default::default())),
            tx_to_localhost_clients: Arc::new(RwLock::new(Default::default())),
            orphan_sessions: Arc::new(RwLock::new(Default::default())),
            pending_peer_connect_signals: Arc::new(RwLock::new(Default::default())),
            peer_username_cache: Arc::new(RwLock::new(Default::default())),
            connecting_usernames: Arc::new(Mutex::new(HashSet::new())),
            io: Arc::new(RwLock::new(Some(io))),
        }
    }
}

impl<T: IOInterface, R: Ratchet> CitadelWorkspaceService<T, R> {
    pub fn new(io: T) -> Self {
        io.into()
    }

    pub fn remote(&self) -> &NodeRemote<R> {
        self.remote.as_ref().expect("Kernel not loaded")
    }
}

impl<R: Ratchet> CitadelWorkspaceService<TcpIOInterface, R> {
    pub async fn new_tcp(
        bind_address: SocketAddr,
    ) -> std::io::Result<CitadelWorkspaceService<TcpIOInterface, R>> {
        Ok(TcpIOInterface::new(bind_address).await?.into())
    }

    #[cfg(feature = "websockets")]
    pub async fn new_websocket(
        bind_address: SocketAddr,
    ) -> std::io::Result<CitadelWorkspaceService<WebSocketInterface, R>> {
        let ws_server_io = WebSocketInterface::new(bind_address).await?;
        Ok(ws_server_io.into())
    }
}

impl<R: Ratchet> CitadelWorkspaceService<InMemoryInterface, R> {
    /// Generates an in-memory service connector and kernel. This is useful for programs that do not need
    /// networking to connect between the application and the internal service
    pub fn new_in_memory() -> (
        InternalServiceConnector<InMemoryInterface>,
        CitadelWorkspaceService<InMemoryInterface, R>,
    ) {
        let (tx_to_consumer, rx_from_consumer) = tokio::sync::mpsc::unbounded_channel();
        let (tx_to_svc, rx_from_svc) = tokio::sync::mpsc::unbounded_channel();
        let connector = InternalServiceConnector {
            sink: WrappedSink {
                inner: InMemorySink(tx_to_svc),
            },
            stream: WrappedStream {
                inner: InMemoryStream(rx_from_consumer),
            },
        };
        let kernel = InMemoryInterface {
            sink: Some(tx_to_consumer),
            stream: Some(rx_from_svc),
        }
        .into();
        (connector, kernel)
    }
}

/// Wrapper around PeerChannelSendHalf that allows cloning for async-safe access.
/// This enables us to drop the RwLock on server_connection_map before awaiting sends.
pub type AsyncSink<R> = Arc<tokio::sync::Mutex<PeerChannelSendHalf<R>>>;

#[allow(dead_code)]
pub struct Connection<R: Ratchet> {
    pub sink_to_server: AsyncSink<R>,
    pub client_server_remote: ClientServerRemote<R>,
    pub peers: HashMap<u64, PeerConnection<R>>,
    pub(crate) associated_localhost_connection: Arc<AtomicUuid>,
    pub c2s_file_transfer_handlers: HashMap<ObjectId, Option<ObjectTransferHandler>>,
    pub groups: HashMap<MessageGroupKey, GroupConnection>,
    pub username: String,
    pub server_address: String,
}

#[allow(dead_code)]
pub struct PeerConnection<R: Ratchet> {
    pub sink: AsyncSink<R>,
    /// Optional PeerRemote for advanced operations (file transfers, etc.)
    /// May be None for acceptor-side connections where we only have the channel.
    remote: Option<PeerRemote<R>>,
    handler_map: HashMap<ObjectId, Option<ObjectTransferHandler>>,
    associated_localhost_connection: Arc<AtomicUuid>,
}

#[allow(dead_code)]
pub struct GroupConnection {
    key: MessageGroupKey,
    tx: GroupChannelSendHalf,
    cid: u64,
}

impl<R: Ratchet> Connection<R> {
    fn new(
        sink: PeerChannelSendHalf<R>,
        client_server_remote: ClientServerRemote<R>,
        associated_tcp_connection: Arc<AtomicUuid>,
        username: String,
        server_address: String,
    ) -> Self {
        Connection {
            peers: HashMap::new(),
            sink_to_server: Arc::new(tokio::sync::Mutex::new(sink)),
            client_server_remote,
            associated_localhost_connection: associated_tcp_connection,
            c2s_file_transfer_handlers: HashMap::new(),
            username,
            groups: HashMap::new(),
            server_address,
        }
    }

    fn add_peer_connection(
        &mut self,
        peer_cid: u64,
        sink: PeerChannelSendHalf<R>,
        remote: PeerRemote<R>,
    ) {
        self.peers.insert(
            peer_cid,
            PeerConnection {
                sink: Arc::new(tokio::sync::Mutex::new(sink)),
                remote: Some(remote),
                handler_map: HashMap::new(),
                associated_localhost_connection: self.associated_localhost_connection.clone(),
            },
        );
    }

    /// Add a peer connection without a PeerRemote (for acceptor-side channels)
    pub fn add_peer_connection_channel_only(
        &mut self,
        peer_cid: u64,
        sink: PeerChannelSendHalf<R>,
    ) {
        self.peers.insert(
            peer_cid,
            PeerConnection {
                sink: Arc::new(tokio::sync::Mutex::new(sink)),
                remote: None,
                handler_map: HashMap::new(),
                associated_localhost_connection: self.associated_localhost_connection.clone(),
            },
        );
    }

    fn clear_peer_connection(&mut self, peer_cid: u64) -> Option<PeerConnection<R>> {
        self.peers.remove(&peer_cid)
    }

    fn add_object_transfer_handler(
        &mut self,
        peer_cid: u64,
        object_id: ObjectId,
        handler: Option<ObjectTransferHandler>,
    ) {
        if self.session_cid() == peer_cid {
            // C2S
            self.c2s_file_transfer_handlers.insert(object_id, handler);
        } else {
            // P2P
            if let Some(peer_connection) = self.peers.get_mut(&peer_cid) {
                peer_connection.handler_map.insert(object_id, handler);
            }
        }
    }

    pub fn add_group_channel(
        &mut self,
        group_key: MessageGroupKey,
        group_channel: GroupConnection,
    ) {
        self.groups.insert(group_key, group_channel);
    }

    fn take_file_transfer_handle(
        &mut self,
        peer_cid: u64,
        object_id: ObjectId,
    ) -> Option<Option<ObjectTransferHandler>> {
        if self.session_cid() == peer_cid {
            // C2S
            self.c2s_file_transfer_handlers.remove(&object_id)
        } else {
            // P2P
            let peer_connection = self.peers.get_mut(&peer_cid)?;
            peer_connection.handler_map.remove(&object_id)
        }
    }

    /// Returns the CID of this C2S connection
    fn session_cid(&self) -> u64 {
        self.client_server_remote.user().get_session_cid()
    }
}

impl<R: Ratchet> Drop for Connection<R> {
    fn drop(&mut self) {
        let remote = self.client_server_remote.clone();
        // Filter out peers without remotes (acceptor-side connections)
        let peers: Vec<_> = self
            .peers
            .drain()
            .into_iter()
            .filter_map(|(_k, v)| v.remote)
            .collect();
        drop(tokio::spawn(async move {
            for peer in peers {
                let _ = peer.disconnect().await;
            }

            let _ = remote.disconnect().await;
        }));
    }
}

impl<T: IOInterface, R: Ratchet> CitadelWorkspaceService<T, R> {
    // Query SDK for active sessions. Useful for when determining if there is asymmetry between the inner protocol
    // and the internal service
    pub async fn client_or_peer_in_protocol(
        &self,
        cid: u64,
        peer_cid: Option<u64>,
    ) -> Result<bool, NetworkError> {
        self.remote().sessions().await.map(|sessions| {
            let conn = sessions.sessions.iter().find(|sess| sess.cid == cid);
            if let Some(conn) = conn {
                if let Some(peer_cid) = peer_cid {
                    conn.connections
                        .iter()
                        .any(|conn| conn.peer_cid.unwrap_or(0) == peer_cid)
                } else {
                    // C2S connected already
                    true
                }
            } else {
                false
            }
        })
    }

    fn clear_peer_connection(
        &self,
        implicated_cid: u64,
        peer_cid: u64,
    ) -> Option<PeerConnection<R>> {
        self.server_connection_map
            .write()
            .get_mut(&implicated_cid)?
            .clear_peer_connection(peer_cid)
    }
}

#[async_trait]
impl<T: IOInterface + Sync, R: Ratchet> NetKernel<R> for CitadelWorkspaceService<T, R> {
    fn load_remote(&mut self, node_remote: NodeRemote<R>) -> Result<(), NetworkError> {
        self.remote = Some(node_remote);
        Ok(())
    }

    async fn on_start(&self) -> Result<(), NetworkError> {
        let this = self.clone();
        let remote = self.remote.clone().unwrap();
        let remote_for_closure = remote.clone();
        let mut io = self.io.write().take().expect("Already called");

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let tcp_connection_map = &self.tx_to_localhost_clients;
        let server_connection_map = &self.server_connection_map;

        let listener_task = async move {
            while let Some((sink, stream)) = io.next_connection().await {
                let (tx1, rx1) = tokio::sync::mpsc::unbounded_channel::<InternalServiceResponse>();
                let id = Uuid::new_v4();
                tcp_connection_map.write().insert(id, tx1);
                io.spawn_connection_handler(
                    sink,
                    stream,
                    tx.clone(),
                    rx1,
                    id,
                    tcp_connection_map.clone(),
                    server_connection_map.clone(),
                    self.orphan_sessions.clone(),
                );
            }
            Ok(())
        };

        let _server_connection_map = &self.server_connection_map;

        let inbound_command_task = async move {
            while let Some((command, conn_id)) = rx.recv().await {
                let this = this.clone();

                let task = async move {
                    if let Some(HandledRequestResult { response, uuid }) =
                        handle_request(&this, conn_id, command).await
                    {
                        if let Err(err) = send_response_to_tcp_client(
                            &this.tx_to_localhost_clients,
                            response,
                            uuid,
                        ) {
                            // The TCP connection no longer exists. Delete it from both maps
                            error!(target: "citadel", "Failed to send response to TCP client: {err:?}");
                            this.tx_to_localhost_clients.write().remove(&uuid);
                            this.server_connection_map.write().retain(|_, v| {
                                v.associated_localhost_connection.load(Ordering::Relaxed) != uuid
                            });
                        }
                    }
                };

                // Spawn the task to allow for parallel request handling
                drop(tokio::task::spawn(task));
            }
            Ok(())
        };

        let res = tokio::select! {
            res0 = listener_task => res0,
            res1 = inbound_command_task => res1,
        };

        warn!(target: "citadel", "Shutting down service because a critical task finished. {res:?}");
        remote_for_closure.shutdown().await?;
        res
    }

    async fn on_node_event_received(&self, message: NodeResult<R>) -> Result<(), NetworkError> {
        responses::handle_node_result(self, message).await
    }

    async fn on_stop(&mut self) -> Result<(), NetworkError> {
        Ok(())
    }
}

fn send_response_to_tcp_client(
    hash_map: &Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    response: InternalServiceResponse,
    uuid: Uuid,
) -> Result<(), NetworkError> {
    let map = hash_map.read();

    match map.get(&uuid) {
        Some(sender) => sender.send(response).map_err(|err| {
            NetworkError::Generic(format!("Failed to send response to TCP client: {err:?}"))
        }),
        None => {
            // Log a warning instead of returning an error that crashes the service
            warn!(target: "citadel", "TCP connection not found: {uuid:?} - response will be dropped");
            Ok(())
        }
    }
}

// TODO: return scoped wrapper type
fn create_client_server_remote<R: Ratchet>(
    conn_type: VirtualTargetType,
    remote: NodeRemote<R>,
    security_settings: SessionSecuritySettings,
) -> ClientServerRemote<R> {
    ClientServerRemote::new(conn_type, remote, security_settings, None, None)
}

pub(crate) async fn sink_send_payload<T: IOInterface>(
    payload: InternalServiceResponse,
    sink: &mut T::Sink,
) -> Result<(), <T::Sink as Sink<InternalServicePayload>>::Error> {
    sink.send(InternalServicePayload::Response(payload)).await
}

pub(crate) fn send_to_kernel(
    request: InternalServiceRequest,
    sender: &UnboundedSender<(InternalServiceRequest, Uuid)>,
    conn_id: Uuid,
) -> Result<(), NetworkError> {
    sender.send((request, conn_id))?;
    Ok(())
}

fn spawn_tick_updater<R: Ratchet>(
    object_transfer_handler: ObjectTransferHandler,
    implicated_cid: u64,
    peer_cid: Option<u64>,
    server_connection_map: &mut HashMap<u64, Connection<R>>,
    tcp_connection_map: Arc<RwLock<HashMap<Uuid, UnboundedSender<InternalServiceResponse>>>>,
    request_id: Option<Uuid>,
) {
    let mut handle_inner = object_transfer_handler.inner;
    if let Some(connection) = server_connection_map.get_mut(&implicated_cid) {
        let uuid = connection
            .associated_localhost_connection
            .load(Ordering::Relaxed);
        let request_id = Some(request_id.unwrap_or(uuid));
        let sender_status_updater = async move {
            while let Some(status) = handle_inner.next().await {
                let status_message = status.clone();
                // Clone the sender outside the lock to avoid holding lock across send
                let sender = { tcp_connection_map.read().get(&uuid).cloned() };
                match sender {
                    Some(entry) => {
                        let message = InternalServiceResponse::FileTransferTickNotification(
                            FileTransferTickNotification {
                                cid: implicated_cid,
                                peer_cid,
                                status: status_message,
                                request_id,
                            },
                        );
                        match entry.send(message.clone()) {
                            Ok(_res) => {
                                info!(target: "citadel", "File Transfer Status Tick Sent {status:?}");
                            }
                            Err(err) => {
                                warn!(target: "citadel", "File Transfer Status Tick Not Sent: {err:?}");
                            }
                        }

                        if matches!(
                            status,
                            ObjectTransferStatus::TransferComplete
                                | ObjectTransferStatus::ReceptionComplete
                        ) {
                            info!(target: "citadel", "File Transfer Completed - Ending Tick Updater");
                            break;
                        }
                    }
                    None => {
                        warn!(target:"citadel","Connection not found during File Transfer Status Tick")
                    }
                }
            }
            info!(target:"citadel", "Spawned Tick Updater has ended for {implicated_cid:?}");
        };
        tokio::task::spawn(sender_status_updater);
    } else {
        info!(target: "citadel", "tick_updater: Server Connection Not Found")
    }
}
