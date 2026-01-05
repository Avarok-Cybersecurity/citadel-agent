use bytes::BytesMut;
use citadel_internal_service_macros::{Cid, IsError, IsNotification, RequestId};
use citadel_types::crypto::PreSharedKey;
pub use citadel_types::prelude::{
    ConnectMode, MemberState, MessageGroupKey, ObjectId, ObjectTransferStatus, SecBuffer,
    SecurityLevel, SessionSecuritySettings, TransferType, UdpMode, UserIdentifier,
    VirtualObjectMetadata,
};
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

pub fn bytes_debug_fmt<T: std::fmt::Debug + AsRef<[u8]>>(
    val: &T,
    f: &mut std::fmt::Formatter,
) -> std::fmt::Result {
    const SAMPLE_ENDS_COUNT: usize = 5;
    let slice = val.as_ref();
    let len = slice.len();
    if len <= (SAMPLE_ENDS_COUNT * 2) {
        return write!(f, "{{BytesLike(len: {len}. values: {slice:?})}}");
    }

    // Get the first and last 5 bytes
    let sample_ending_boundary = len.saturating_sub(SAMPLE_ENDS_COUNT);
    let first_bytes: &[u8] = &slice[..SAMPLE_ENDS_COUNT];
    let last_bytes: &[u8] = &slice[sample_ending_boundary..];

    write!(f, "{{BytesLike(len: {len}. First {SAMPLE_ENDS_COUNT} bytes: {first_bytes:?}. Last {SAMPLE_ENDS_COUNT} bytes: {last_bytes:?})}}")
}

pub fn map_debug_fmt<T, K, V>(map: &T, f: &mut std::fmt::Formatter) -> std::fmt::Result
where
    T: ?Sized,
    for<'a> &'a T: IntoIterator<Item = (&'a K, &'a V)>,
    K: std::fmt::Display,
    V: std::fmt::Debug + AsRef<[u8]>,
{
    write!(f, "{{MapLike: ")?;

    // Use a peekable iterator to handle the trailing comma correctly.
    let mut iter = map.into_iter().peekable();

    while let Some((k, v)) = iter.next() {
        write!(f, "(K: {k}, V: ")?;
        // `v` is a `&V`. Because `V: AsRef<[u8]>`, `&V` also implements `AsRef<[u8]>`.
        // So we can pass `v` directly to our helper.
        bytes_debug_fmt(v, f)?;
        write!(f, ")")?;

        // Only write a comma if this is not the last item.
        if iter.peek().is_some() {
            write!(f, ", ")?;
        }
    }

    write!(f, "}}")
}

/// Thread-safe wrapper for UUID that can be atomically updated
#[derive(Debug)]
pub struct AtomicUuid {
    high: AtomicU64,
    low: AtomicU64,
}

impl AtomicUuid {
    pub fn new(uuid: Uuid) -> Self {
        let bytes = uuid.as_bytes();
        let high = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
        let low = u64::from_be_bytes(bytes[8..16].try_into().unwrap());

        Self {
            high: AtomicU64::new(high),
            low: AtomicU64::new(low),
        }
    }

    pub fn load(&self, ordering: Ordering) -> Uuid {
        let high = self.high.load(ordering);
        let low = self.low.load(ordering);

        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&high.to_be_bytes());
        bytes[8..16].copy_from_slice(&low.to_be_bytes());

        Uuid::from_bytes(bytes)
    }

    pub fn store(&self, uuid: Uuid, ordering: Ordering) {
        let bytes = uuid.as_bytes();
        let high = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
        let low = u64::from_be_bytes(bytes[8..16].try_into().unwrap());

        self.high.store(high, ordering);
        self.low.store(low, ordering);
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConnectSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConnectFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct RegisterSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct RegisterFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ServiceConnectionAccepted {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct MessageSendSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct MessageSendFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct MessageNotification {
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    pub message: BytesMut,
    pub cid: u64,
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DisconnectNotification {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DisconnectFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DeregisterSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DeregisterFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SendFileRequestSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SendFileRequestFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DownloadFileSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DownloadFileFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DeleteVirtualFileSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct DeleteVirtualFileFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PickFileSuccess {
    pub cid: u64,
    /// The full path to the selected file
    #[cfg_attr(feature = "typescript", ts(type = "string"))]
    pub file_path: PathBuf,
    /// The file name (without path)
    pub file_name: String,
    /// The file size in bytes
    pub file_size: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PickFileFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerConnectSuccess {
    pub cid: u64,
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerConnectFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerConnectAcceptSuccess {
    pub cid: u64,
    pub peer_cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerConnectAcceptFailure {
    pub cid: u64,
    pub peer_cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerDisconnectSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerDisconnectFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerConnectNotification {
    pub cid: u64,
    pub peer_cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub session_security_settings: SessionSecuritySettings,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub udp_mode: UdpMode,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerRegisterNotification {
    pub cid: u64,
    pub peer_cid: u64,
    pub peer_username: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerRegisterSuccess {
    pub cid: u64,
    pub peer_cid: u64,
    pub peer_username: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerRegisterFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupChannelCreateSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupChannelCreateFailure {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupBroadcastHandleFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupCreateSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupCreateFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupLeaveSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupLeaveFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupEndSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupEndFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupEndNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub success: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupLeaveNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub success: bool,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageNotification {
    pub cid: u64,
    pub peer_cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = bytes_debug_fmt)]
    pub message: BytesMut,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub success: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMessageFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupInviteNotification {
    pub cid: u64,
    pub peer_cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupInviteSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupInviteFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRespondRequestSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRespondRequestFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMembershipResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub success: bool,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRequestJoinPendingNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub result: Result<(), String>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupDisconnectNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupKickSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupKickFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupListGroupsSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    #[cfg_attr(feature = "typescript", ts(type = "any[] | null"))]
    pub group_list: Option<Vec<MessageGroupKey>>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupListGroupsFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupListGroupsResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any[] | null"))]
    pub group_list: Option<Vec<MessageGroupKey>>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupJoinRequestNotification {
    pub cid: u64,
    pub peer_cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRequestJoinAcceptResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRequestJoinDeclineResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRequestJoinSuccess {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupRequestJoinFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GroupMemberStateChangeNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub group_key: MessageGroupKey,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub state: MemberState,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBGetKVSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub key: String,
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    #[debug(with = bytes_debug_fmt)]
    pub value: Vec<u8>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBGetKVFailure {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBSetKVSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub key: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBSetKVFailure {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBDeleteKVSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub key: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBDeleteKVFailure {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBGetAllKVSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    #[cfg_attr(feature = "typescript", ts(type = "Record<string, number[]>"))]
    #[debug(with = map_debug_fmt)]
    pub map: HashMap<String, Vec<u8>>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBGetAllKVFailure {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBClearAllKVSuccess {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerInformation {
    pub cid: u64,
    pub online_status: bool,
    pub name: Option<String>,
    pub username: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ListAllPeersResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "Record<string, PeerInformation>"))]
    pub peer_information: HashMap<u64, PeerInformation>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ListAllPeersFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ListRegisteredPeersFailure {
    pub cid: u64,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ListRegisteredPeersResponse {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "Record<string, PeerInformation>"))]
    pub peers: HashMap<u64, PeerInformation>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct LocalDBClearAllKVFailure {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    pub message: String,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct GetSessionsResponse {
    pub cid: u64,
    pub sessions: Vec<SessionInformation>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct FileTransferRequestNotification {
    pub cid: u64,
    pub peer_cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub metadata: VirtualObjectMetadata,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct FileTransferStatusNotification {
    pub cid: u64,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub object_id: ObjectId,
    pub success: bool,
    pub response: bool,
    pub message: Option<String>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct FileTransferTickNotification {
    pub cid: u64,
    pub peer_cid: Option<u64>,
    #[cfg_attr(feature = "typescript", ts(type = "any"))]
    pub status: ObjectTransferStatus,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug, Clone, IsError, IsNotification, RequestId, Cid)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum InternalServiceResponse {
    ConnectSuccess(ConnectSuccess),
    ConnectFailure(ConnectFailure),
    RegisterSuccess(RegisterSuccess),
    RegisterFailure(RegisterFailure),
    ServiceConnectionAccepted(ServiceConnectionAccepted),
    MessageSendSuccess(MessageSendSuccess),
    MessageSendFailure(MessageSendFailure),
    MessageNotification(MessageNotification),
    DisconnectNotification(DisconnectNotification),
    DisconnectFailure(DisconnectFailure),
    DeregisterSuccess(DeregisterSuccess),
    DeregisterFailure(DeregisterFailure),
    SendFileRequestSuccess(SendFileRequestSuccess),
    SendFileRequestFailure(SendFileRequestFailure),
    FileTransferRequestNotification(FileTransferRequestNotification),
    FileTransferStatusNotification(FileTransferStatusNotification),
    FileTransferTickNotification(FileTransferTickNotification),
    DownloadFileSuccess(DownloadFileSuccess),
    DownloadFileFailure(DownloadFileFailure),
    DeleteVirtualFileSuccess(DeleteVirtualFileSuccess),
    DeleteVirtualFileFailure(DeleteVirtualFileFailure),
    PickFileSuccess(PickFileSuccess),
    PickFileFailure(PickFileFailure),
    PeerConnectSuccess(PeerConnectSuccess),
    PeerConnectFailure(PeerConnectFailure),
    PeerConnectAcceptSuccess(PeerConnectAcceptSuccess),
    PeerConnectAcceptFailure(PeerConnectAcceptFailure),
    PeerConnectNotification(PeerConnectNotification),
    PeerRegisterNotification(PeerRegisterNotification),
    PeerDisconnectSuccess(PeerDisconnectSuccess),
    PeerDisconnectFailure(PeerDisconnectFailure),
    PeerRegisterSuccess(PeerRegisterSuccess),
    PeerRegisterFailure(PeerRegisterFailure),
    GroupChannelCreateSuccess(GroupChannelCreateSuccess),
    GroupChannelCreateFailure(GroupChannelCreateFailure),
    GroupBroadcastHandleFailure(GroupBroadcastHandleFailure),
    GroupCreateSuccess(GroupCreateSuccess),
    GroupCreateFailure(GroupCreateFailure),
    GroupLeaveSuccess(GroupLeaveSuccess),
    GroupLeaveFailure(GroupLeaveFailure),
    GroupLeaveNotification(GroupLeaveNotification),
    GroupEndSuccess(GroupEndSuccess),
    GroupEndFailure(GroupEndFailure),
    GroupEndNotification(GroupEndNotification),
    GroupMessageNotification(GroupMessageNotification),
    GroupMessageResponse(GroupMessageResponse),
    GroupMessageSuccess(GroupMessageSuccess),
    GroupMessageFailure(GroupMessageFailure),
    GroupInviteNotification(GroupInviteNotification),
    GroupInviteSuccess(GroupInviteSuccess),
    GroupInviteFailure(GroupInviteFailure),
    GroupRespondRequestSuccess(GroupRespondRequestSuccess),
    GroupRespondRequestFailure(GroupRespondRequestFailure),
    GroupMembershipResponse(GroupMembershipResponse),
    GroupRequestJoinPendingNotification(GroupRequestJoinPendingNotification),
    GroupDisconnectNotification(GroupDisconnectNotification),
    GroupKickSuccess(GroupKickSuccess),
    GroupKickFailure(GroupKickFailure),
    GroupListGroupsSuccess(GroupListGroupsSuccess),
    GroupListGroupsFailure(GroupListGroupsFailure),
    GroupListGroupsResponse(GroupListGroupsResponse),
    GroupJoinRequestNotification(GroupJoinRequestNotification),
    GroupRequestJoinAcceptResponse(GroupRequestJoinAcceptResponse),
    GroupRequestJoinDeclineResponse(GroupRequestJoinDeclineResponse),
    GroupRequestJoinSuccess(GroupRequestJoinSuccess),
    GroupRequestJoinFailure(GroupRequestJoinFailure),
    GroupMemberStateChangeNotification(GroupMemberStateChangeNotification),
    LocalDBGetKVSuccess(LocalDBGetKVSuccess),
    LocalDBGetKVFailure(LocalDBGetKVFailure),
    LocalDBSetKVSuccess(LocalDBSetKVSuccess),
    LocalDBSetKVFailure(LocalDBSetKVFailure),
    LocalDBDeleteKVSuccess(LocalDBDeleteKVSuccess),
    LocalDBDeleteKVFailure(LocalDBDeleteKVFailure),
    LocalDBGetAllKVSuccess(LocalDBGetAllKVSuccess),
    LocalDBGetAllKVFailure(LocalDBGetAllKVFailure),
    LocalDBClearAllKVSuccess(LocalDBClearAllKVSuccess),
    LocalDBClearAllKVFailure(LocalDBClearAllKVFailure),
    GetSessionsResponse(GetSessionsResponse),
    GetAccountInformationResponse(Accounts),
    ListAllPeersResponse(ListAllPeersResponse),
    ListAllPeersFailure(ListAllPeersFailure),
    ListRegisteredPeersResponse(ListRegisteredPeersResponse),
    ListRegisteredPeersFailure(ListRegisteredPeersFailure),
    ConnectionManagementSuccess(ConnectionManagementSuccess),
    ConnectionManagementFailure(ConnectionManagementFailure),
    /// Results from a batched request, in the same order as input commands
    BatchedResponse(BatchedResponseData),
}

#[derive(Serialize, Deserialize, Debug, Clone, RequestId)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum InternalServiceRequest {
    Connect {
        request_id: Uuid,
        username: String,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        password: SecBuffer,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        connect_mode: ConnectMode,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        udp_mode: UdpMode,
        #[cfg_attr(
            feature = "typescript",
            ts(type = "{ secs: number; nanos: number } | null")
        )]
        keep_alive_timeout: Option<Duration>,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        session_security_settings: SessionSecuritySettings,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        server_password: Option<PreSharedKey>,
    },
    Register {
        request_id: Uuid,
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        server_addr: SocketAddr,
        full_name: String,
        username: String,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        proposed_password: SecBuffer,
        connect_after_register: bool,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        session_security_settings: SessionSecuritySettings,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        server_password: Option<PreSharedKey>,
    },
    Message {
        request_id: Uuid,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(with = bytes_debug_fmt)]
        message: Vec<u8>,
        cid: u64,
        peer_cid: Option<u64>,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        security_level: SecurityLevel,
    },
    Disconnect {
        request_id: Uuid,
        cid: u64,
    },
    /// Deregister from the server - permanently removes the account
    Deregister {
        request_id: Uuid,
        cid: u64,
    },
    SendFile {
        request_id: Uuid,
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        source: PathBuf,
        cid: u64,
        peer_cid: Option<u64>,
        chunk_size: Option<usize>,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        transfer_type: TransferType,
    },
    RespondFileTransfer {
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        object_id: ObjectId,
        accept: bool,
        #[cfg_attr(feature = "typescript", ts(type = "string | null"))]
        download_location: Option<PathBuf>,
        request_id: Uuid,
    },
    DownloadFile {
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        virtual_directory: PathBuf,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        security_level: Option<SecurityLevel>,
        delete_on_pull: bool,
        cid: u64,
        peer_cid: Option<u64>,
        request_id: Uuid,
    },
    DeleteVirtualFile {
        #[cfg_attr(feature = "typescript", ts(type = "string"))]
        virtual_directory: PathBuf,
        cid: u64,
        peer_cid: Option<u64>,
        request_id: Uuid,
    },
    /// Opens a native file picker dialog to select a file.
    /// Returns the full file path, name, and size.
    /// This runs on the native internal-service (not WASM) so it has full filesystem access.
    PickFile {
        request_id: Uuid,
        cid: u64,
        /// Optional title for the file picker dialog
        title: Option<String>,
        /// Optional list of allowed file extensions (e.g., ["pdf", "txt"])
        #[cfg_attr(feature = "typescript", ts(type = "string[] | null"))]
        allowed_extensions: Option<Vec<String>>,
    },
    ListAllPeers {
        request_id: Uuid,
        cid: u64,
    },
    ListRegisteredPeers {
        request_id: Uuid,
        cid: u64,
    },
    PeerConnect {
        request_id: Uuid,
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        udp_mode: UdpMode,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        session_security_settings: SessionSecuritySettings,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        peer_session_password: Option<PreSharedKey>,
    },
    PeerDisconnect {
        request_id: Uuid,
        cid: u64,
        peer_cid: u64,
    },
    /// Accept an incoming P2P connection request from a peer.
    /// This is sent in response to PeerConnectNotification to complete the handshake.
    PeerConnectAccept {
        request_id: Uuid,
        /// CID of the local session accepting the connection
        cid: u64,
        /// CID of the peer who initiated the connection
        peer_cid: u64,
        /// Accept (true) or decline (false) the connection
        accept: bool,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        udp_mode: UdpMode,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        session_security_settings: SessionSecuritySettings,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        peer_session_password: Option<PreSharedKey>,
    },
    PeerRegister {
        request_id: Uuid,
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        session_security_settings: SessionSecuritySettings,
        connect_after_register: bool,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        peer_session_password: Option<PreSharedKey>,
    },
    LocalDBGetKV {
        request_id: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
        key: String,
    },
    LocalDBSetKV {
        request_id: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
        key: String,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(with = bytes_debug_fmt)]
        value: Vec<u8>,
    },
    LocalDBDeleteKV {
        request_id: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
        key: String,
    },
    LocalDBGetAllKV {
        request_id: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
    },
    LocalDBClearAllKV {
        request_id: Uuid,
        cid: u64,
        peer_cid: Option<u64>,
    },
    GetSessions {
        request_id: Uuid,
    },
    GetAccountInformation {
        request_id: Uuid,
        cid: Option<u64>,
    },
    GroupCreate {
        cid: u64,
        request_id: Uuid,
        #[cfg_attr(feature = "typescript", ts(type = "any[] | null"))]
        initial_users_to_invite: Option<Vec<UserIdentifier>>,
    },
    GroupLeave {
        cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    GroupEnd {
        cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    GroupMessage {
        cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
        #[debug(with = bytes_debug_fmt)]
        message: BytesMut,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    GroupInvite {
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    GroupRespondRequest {
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        response: bool,
        request_id: Uuid,
        invitation: bool,
    },
    GroupKick {
        cid: u64,
        peer_cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    GroupListGroupsFor {
        cid: u64,
        peer_cid: Option<u64>,
        request_id: Uuid,
    },
    GroupRequestJoin {
        cid: u64,
        #[cfg_attr(feature = "typescript", ts(type = "any"))]
        group_key: MessageGroupKey,
        request_id: Uuid,
    },
    ConnectionManagement {
        request_id: Uuid,
        management_command: ConfigCommand,
    },
    /// Execute multiple requests in parallel, returning results in the same order as input.
    /// This enables single-roundtrip batch operations for efficiency.
    Batched {
        request_id: Uuid,
        /// The list of commands to execute in parallel
        commands: Vec<InternalServiceRequest>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConnectionManagementSuccess {
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
}

/// Response from a batched request containing results in the same order as input commands
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct BatchedResponseData {
    /// CID is 0 for batched responses (batch is not tied to a single session)
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub results: Vec<InternalServiceResponse>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ConnectionManagementFailure {
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum ConfigCommand {
    SetConnectionOrphan {
        allow_orphan_sessions: bool,
    },
    ClaimSession {
        session_cid: u64,
        only_if_orphaned: bool,
    },
    DisconnectOrphan {
        session_cid: Option<u64>,
    },
    /// Release a session, marking it as orphaned without disconnecting.
    /// Called when the last browser tab with this CID closes.
    /// The session stays in server_connection_map but becomes immediately claimable.
    ReleaseSession {
        session_cid: u64,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct SessionInformation {
    pub cid: u64,
    pub username: String,
    pub server_address: String,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "Record<string, PeerSessionInformation>")
    )]
    pub peer_connections: HashMap<u64, PeerSessionInformation>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct Accounts {
    pub cid: u64,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "Record<string, AccountInformation>")
    )]
    pub accounts: HashMap<u64, AccountInformation>,
    pub request_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct AccountInformation {
    pub username: String,
    pub full_name: String,
    #[cfg_attr(
        feature = "typescript",
        ts(type = "Record<string, PeerSessionInformation>")
    )]
    pub peers: HashMap<u64, PeerSessionInformation>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct PeerSessionInformation {
    pub cid: u64,
    pub peer_cid: u64,
    pub peer_username: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub enum InternalServicePayload {
    Request(InternalServiceRequest),
    Response(InternalServiceResponse),
}

impl From<InternalServiceResponse> for InternalServicePayload {
    fn from(response: InternalServiceResponse) -> Self {
        InternalServicePayload::Response(response)
    }
}

impl From<InternalServiceRequest> for InternalServicePayload {
    fn from(request: InternalServiceRequest) -> Self {
        InternalServicePayload::Request(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_error_derive() {
        let success_response = InternalServiceResponse::ConnectSuccess(ConnectSuccess {
            cid: 0,
            request_id: None,
        });
        let error_response = InternalServiceResponse::ConnectFailure(ConnectFailure {
            cid: 0,
            message: "test".to_string(),
            request_id: None,
        });
        assert!(!success_response.is_error());
        assert!(error_response.is_error());
    }

    #[test]
    fn test_is_notification_derive() {
        let success_response = InternalServiceResponse::ConnectSuccess(ConnectSuccess {
            cid: 0,
            request_id: None,
        });
        let notification_response =
            InternalServiceResponse::PeerRegisterNotification(PeerRegisterNotification {
                cid: 0,
                peer_cid: 0,
                peer_username: "".to_string(),
                request_id: None,
            });
        assert!(!success_response.is_notification());
        assert!(notification_response.is_notification());
    }

    #[test]
    fn test_request_id_derive() {
        let request_id = Uuid::new_v4();
        let request = InternalServiceRequest::Connect {
            request_id,
            username: "test".to_string(),
            password: SecBuffer::from(vec![]),
            connect_mode: ConnectMode::default(),
            udp_mode: UdpMode::Enabled,
            keep_alive_timeout: None,
            session_security_settings: SessionSecuritySettings::default(),
            server_password: None,
        };
        assert_eq!(request.request_id(), Some(&request_id));
    }

    #[test]
    fn test_cid_derive() {
        let cid = 1234;
        let request = InternalServiceResponse::ConnectSuccess(ConnectSuccess {
            cid,
            request_id: None,
        });

        assert_eq!(request.cid(), cid);
    }

    // Test that triggers TypeScript export when running tests with typescript feature
    #[cfg(feature = "typescript")]
    #[test]
    fn trigger_typescript_export() {
        use ts_rs::TS;

        // Access type information to trigger export
        let _ = InternalServiceRequest::name();
        let _ = InternalServiceResponse::name();
        let _ = InternalServicePayload::name();
    }
}
