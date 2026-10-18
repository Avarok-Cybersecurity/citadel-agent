//! Reading a RE-VFS file this session pulled back into the page, chunk by chunk.
//!
//! `DownloadFile` has the SDK save the pulled file on the agent's disk; the page
//! learns only that it landed. `ReadPulledFileChunk` hands its bytes over, the
//! reverse of `StageUploadChunk`: the page names the file the way it pulled it
//! (session, peer, virtual path), never by a local path, and reads it in order.

use crate::bytes_debug_fmt;
use custom_debug::Debug;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "typescript")]
use ts_rs::TS;

/// One chunk of a pulled file. The file is complete when
/// `offset + data.len() == total_size`.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ReadPulledFileChunkSuccess {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub total_size: u64,
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub offset: u64,
    #[debug(with = bytes_debug_fmt)]
    #[cfg_attr(feature = "typescript", ts(type = "number[]"))]
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[cfg_attr(feature = "typescript", ts(export))]
pub struct ReadPulledFileChunkFailure {
    #[cfg_attr(feature = "typescript", ts(type = "bigint"))]
    pub cid: u64,
    pub request_id: Option<Uuid>,
    pub message: String,
}
